use std::{io::Read, path::PathBuf, sync::atomic::Ordering, thread::sleep, time::Duration};

use log::{error, info, warn};
use opencv::{
    core::{Mat, MatTraitConst, MatTraitConstManual, Ptr, Scalar, Size, Vector}, dnn::NetTrait, objdetect::{FaceDetectorYN, FaceRecognizerSF, FaceRecognizerSF_DisType}, prelude::{FaceDetectorYNTrait, FaceRecognizerSFTrait, FaceRecognizerSFTraitConst}, videoio::{self, VideoCapture, VideoCaptureTrait, VideoCaptureTraitConst}
};
use serde::{Deserialize, Serialize};
use windows::{core::HSTRING, Win32::Foundation::E_UNEXPECTED};

use crate::{global::{
    get_global_log_path, set_face_recognition_mode, CAMERA_INDEX, DB_POOL, FACE_RECOG_DELAY, IS_RUN, LIVENESS_ENABLE, LIVENESS_THRESHOLD, MATCH_FAIL_COUNT, MAX_FAIL, MAX_SUCCESS, RETRY_DELAY
}, pipe::Client, utils::{save_mat_as_faceimg, set_last_send_time}};

// 定义摄像头后端类型枚举
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum CameraBackend {
    Any,   // CAP_ANY
    DShow, // CAP_DSHOW
    MSMF,  // CAP_MSMF
    VFW,   // CAP_VFW
}

impl From<CameraBackend> for i32 {
    fn from(backend: CameraBackend) -> Self {
        match backend {
            CameraBackend::Any => videoio::CAP_ANY,
            CameraBackend::DShow => videoio::CAP_DSHOW,
            CameraBackend::MSMF => videoio::CAP_MSMF,
            CameraBackend::VFW => videoio::CAP_VFW,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")] // 适配 JSON 中的驼峰命名
pub struct FaceExtraData {
    /// 面容别名
    pub alias: String,
    /// 置信度阈值
    pub threshold: f32,
    /// 是否在列表页显示图片缩略图
    pub view: bool,
    // 是否锁定面容？为true时不参与判定
    #[serde(default)] // 0.2.0 以下版本的用户没有这一项，默认为false
    pub lock: bool,
    /// 人脸检测置信度阈值
    pub face_detection_threshold: f32,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct FaceDescriptor {
    pub name: String,
    pub feature: Vec<f32>,
}

impl FaceDescriptor {
    // 将 OpenCV 的 Mat 转换为可序列化的结构
    pub fn from_mat(name: &str, feature_mat: &Mat) -> Result<Self, Box<dyn std::error::Error>> {
        // 确保 Mat 是连续的，然后转换为 Vec
        let mut feature_vec: Vec<f32> = vec![0.0f32; feature_mat.total()];
        let data = feature_mat.data_typed::<f32>()?;
        feature_vec.copy_from_slice(data);

        Ok(FaceDescriptor {
            name: name.to_string(),
            feature: feature_vec,
        })
    }

    // 将特征向量还原回 OpenCV Mat
    pub fn to_mat(&self) -> Result<Mat, Box<dyn std::error::Error>> {
        // 从切片创建原始 Mat (默认为 N 行 1 列)
        let m = Mat::from_slice(&self.feature)?;

        // 变换形状为 1 行 128 列
        // reshape 返回的是 Result<BoxedRef<Mat>, ...>
        let m_reshaped = m.reshape(1, 1)?;

        // 使用 try_clone() 进行深拷贝，转回独立的 Mat 对象
        let final_mat = m_reshaped.try_clone()?;

        Ok(final_mat)
    }
}

// 刚锁屏时的预处理
pub fn prepare_before() -> Result<(), String> {
    let pool_guard = DB_POOL.lock().unwrap();
    let pool = pool_guard.as_ref();
    if pool.is_none() {
        return Err(String::from("数据库连接池未初始化，无法进行面容识别"));
    }
    let conn = pool.unwrap().get().map_err(|e| e.to_string())?;
    let result = conn
        .query_row("SELECT COUNT(id) as count FROM faces;", [], |row| {
            row.get::<&str, i32>("count")
        })
        .map_err(|e| format!("查询数据库失败：{:?}", e))?;
    if result > 0 {
        let result: Result<String, _> = conn.query_row(
            "SELECT val FROM options WHERE key = 'is_initialized';",
            [],
            |row| row.get::<&str, String>("val"),
        );
        let is_initialized = match result {
            Ok(val) => val,
            Err(r2d2_sqlite::rusqlite::Error::QueryReturnedNoRows) => String::from("false"),
            Err(e) => {
                return Err(format!("查询数据库失败：{:?}", e));
            }
        };

        if is_initialized == "false" {
            return Err("程序未初始化，无法进行面容识别".to_string());
        }

        // 获取面容识别的类型
        let face_recog_type = conn
            .query_row(
                "SELECT val FROM options WHERE key = 'faceRecogType'",
                (),
                |row| row.get::<&str, String>("val"),
            )
            .unwrap_or(String::from("operation"));

        if face_recog_type != "operation" {
            // 如果是按延迟时间，获取延迟时间
            let time = conn
                .query_row(
                    "SELECT val FROM options WHERE key = 'faceRecogDelay';",
                    [],
                    |row| row.get::<&str, String>("val"),
                )
                .unwrap_or(String::from("10.0"));

            let time_ms: f32 = match time.parse::<f32>() {
                Ok(seconds) => seconds * 1000.0,
                Err(e) => {
                    warn!("秒数字符串转换失败: {}，使用默认值 10000 毫秒", e);
                    10.0 * 1000.0
                }
            };

            FACE_RECOG_DELAY.store(time_ms as u32, Ordering::SeqCst);
        }

        // 设置识别模式
        set_face_recognition_mode(face_recog_type);

        // 读取摄像头索引
        let camera_index = conn
            .query_row("SELECT val FROM options WHERE key = 'camera';", [], |row| {
                row.get::<&str, String>("val")
            })
            .unwrap_or(String::from("0"));

        CAMERA_INDEX.store(camera_index.parse().unwrap_or(0), Ordering::SeqCst);

        // 获取重试时间
        let time = conn
            .query_row(
                "SELECT val FROM options WHERE key = 'retryDelay';",
                [],
                |row| row.get::<&str, String>("val"),
            )
            .unwrap_or(String::from("10.0"));

        let time_ms: f32 = match time.parse::<f32>() {
            Ok(seconds) => seconds * 1000.0,
            Err(e) => {
                warn!("秒数字符串转换失败: {}，使用默认值 10000 毫秒", e);
                10.0 * 1000.0
            }
        };
        RETRY_DELAY.store(time_ms as i32, Ordering::SeqCst);

        // 是否进行活体检测
        let liveness_enabled = conn
            .query_row("SELECT val FROM options WHERE key = 'livenessEnabled';", [], |row| {
                row.get::<&str, String>("val")
            })
            .unwrap_or(String::from("false"));
        LIVENESS_ENABLE.store(liveness_enabled == "true", Ordering::SeqCst);

        // 活体检测阈值
        let liveness_threshold = conn
            .query_row(
                "SELECT val FROM options WHERE key = 'livenessThreshold';",
                [],
                |row| row.get::<&str, String>("val"),
            )
            .unwrap_or(String::from("0.50"));
        LIVENESS_THRESHOLD.store((liveness_threshold.parse().unwrap_or(0.5) * 100.0) as u32, Ordering::SeqCst);
    }

    Ok(())
}

// 开始面容识别
pub fn run_before() {
    // 先打开摄像头
    match open_camera(None, CAMERA_INDEX.load(Ordering::SeqCst)) {
        Ok(camera) => {
            // 摄像头成功打开
            if let Err(e) = run(camera) {
                error!("运行面容解锁失败: {:?}", e);
            };
            set_last_send_time();
            IS_RUN.store(false, Ordering::SeqCst);
        }
        Err(e) => {
            error!("打开摄像头失败 {}", e);
            IS_RUN.store(false, Ordering::SeqCst);
        }
    }
}

// 解锁屏幕
pub fn unlock(user_name: String, password: String) -> windows::core::Result<()> {
    let client = Client::new(HSTRING::from(r"\\.\pipe\MansonWindowsUnlockRustServer"));
    if client.is_err() {
        return Err(windows::core::Error::new(E_UNEXPECTED, "管道不存在"));
    }
    let client = client.unwrap();
    if let Err(e) = crate::pipe::write(client.handle, format!("{}::FaceWinUnlock::{}", user_name, password)) {
        println!("向客户端写入数据失败: {:?}", e);
    }

    Ok(())
}

// 面容识别主程序
fn run(mut camera: VideoCapture) -> Result<bool, String> {
    // 加载模型
    let resource_path = get_global_log_path()
        .join("resources")
        .join("face_detection_yunet_2023mar.onnx");
    let mut detector: Ptr<FaceDetectorYN> = FaceDetectorYN::create(
        resource_path.to_str().unwrap_or(""),
        "",
        Size::new(320, 320), // 初始尺寸，后面会动态更新
        0.9,
        0.3,
        5000,
        0,
        0,
    )
    .map_err(|e| format!("初始化检测器模型失败: {:?}", e))?;

    let resource_path = get_global_log_path()
        .join("resources")
        .join("face_recognition_sface_2021dec.onnx");
    let mut recognizer: Ptr<FaceRecognizerSF> =
        FaceRecognizerSF::create(resource_path.to_str().unwrap_or(""), "", 0, 0)
            .map_err(|e| format!("初始化识别器模型失败: {:?}", e))?;

    let resource_path = get_global_log_path()
        .join("resources")
        .join("face_liveness.onnx");
    let mut liveness_net = opencv::dnn::read_net_from_onnx(resource_path.to_str().unwrap_or(""))
            .map_err(|e| format!("初始化活体检测模型失败: {:?}", e))?;

    let pool_guard = DB_POOL.lock().unwrap();
    let pool = pool_guard.as_ref();
    if pool.is_none() {
        return Err(String::from("数据库连接池未初始化，无法进行面容识别"));
    }
    let conn = pool.unwrap().get().map_err(|e| e.to_string())?;

    // 获取面容数据
    let mut faces = conn
        .prepare("SELECT * FROM faces;")
        .map_err(|e| format!("准备查询面容数据失败：{:?}", e))?;
    let rows = faces
        .query_map([], |row| {
            // 读取基础字段
            let id = row.get::<&str, i32>("id")?;
            let user_name = row.get::<&str, String>("user_name")?;
            let user_pwd = row.get::<&str, String>("user_pwd")?;
            let account_type = row.get::<&str, String>("account_type")?;
            let face_token = row.get::<&str, String>("face_token")?;
            let json_data_str = row.get::<&str, String>("json_data")?;
            let create_time = row.get::<&str, String>("createTime")?;

            // 解析 JSON 字符串为结构体
            let json_data: FaceExtraData = serde_json::from_str(&json_data_str)
                .map_err(|_e| r2d2_sqlite::rusqlite::Error::ExecuteReturnedResults)?;

            // 返回
            Ok((
                id,
                user_name,
                user_pwd,
                account_type,
                face_token,
                json_data,
                create_time,
            ))
        })
        .map_err(|e| format!("查询面容数据失败：{:?}", e))?;

    let mut frame = Mat::default();

    'face: for row in rows {
        let (id, user_name, user_pwd, account_type, mut face_token, json_data, _create_time) =
            row.map_err(|e| format!("获取1条面容数据失败：{:?}", e))?;

        if json_data.lock {
            // 锁定了账户，直接跳过
            continue;
        }

        // 加载数据
        face_token.push_str(".face");
        let path = get_global_log_path().join("faces").join(face_token);
        // 解析面容数据
        let face = load_face_data(&path);
        if face.is_err() {
            error!("加载面容数据失败：{:?}", path);
            continue;
        }

        let face = face.unwrap();
        // 参考面容转换失败，跳过当前用户
        let dst_feature = face.to_mat();
        if dst_feature.is_err() {
            error!("{}, 转换参考面容数据失败：{:?}", json_data.alias, path);
            continue;
        }
        let dst_feature = dst_feature.unwrap();

        let mut success_count = 0;
        let mut fail_count = 0;

        loop {
            // 读取一帧，摄像头的操作一旦失败，必须退出函数
            frame =
                read_mat_from_camera(&mut camera).map_err(|e| format!("摄像头读取失败: {}", e))?;
            // 提取特征点
            let (aligned, cur_feature) = match get_feature(
                &frame,
                json_data.face_detection_threshold,
                &mut detector,
                &mut recognizer,
            ) {
                Ok(feature) => feature,
                Err(e) => {
                    let err_msg = format!("特征提取失败: {}", e);
                    if err_msg.contains("未检测到人脸") {
                        // 未检测到人脸不动
                        sleep(Duration::from_millis(200));
                        continue;
                    } else {
                        // 其他错误退出整个函数
                        return Err(err_msg);
                    }
                }
            };

            // 如果启用了活体检测，进行活体检测
            if LIVENESS_ENABLE.load(Ordering::SeqCst) {
                // 图像预处理
                // blob_from_image 自动完成: 缩放、中心裁剪、BGR转RGB、归一化
                let blob = opencv::dnn::blob_from_image(
                    &aligned,
                    2.0 / 255.0,              // 缩放比例 (1/127.5)
                    Size::new(112, 112), // 尺寸
                    Scalar::new(127.5, 127.5, 127.5, 0.0), // 减去均值
                    true,                     // swapRB: BGR -> RGB
                    false,                    // crop
                    opencv::core::CV_32F      // ddepth
                ).map_err(|e| format!("创建 Blob 失败: {:?}", e))?;

                // 执行活体检测推理
                liveness_net.set_input(&blob, "", 1.0, Scalar::default()).map_err(|e| format!("设置输入失败: {:?}", e))?;
                let mut outputs = Vector::<Mat>::new();
                liveness_net.forward(&mut outputs, &Vector::from_iter(vec![""]))
                    .map_err(|e| format!("执行推理失败: {:?}", e))?;

                let output_mat = outputs.get(0).map_err(|_| format!("无输出"))?;
                let data: &[f32] = output_mat.data_typed().map_err(|e| format!("获取数据失败: {:?}", e))?;

                let mut is_live = false;
                let mut real_prob = 0.0;
                
                if data.len() >= 2 {
                    real_prob = data[1];
                    is_live = real_prob >= LIVENESS_THRESHOLD.load(Ordering::SeqCst) as f32 / 100.0;            
                } else {
                    let score = data[0];
                    real_prob = 1.0 - score;
                    is_live = real_prob >= LIVENESS_THRESHOLD.load(Ordering::SeqCst) as f32 / 100.0;
                }

                if !is_live {
                    // 活体检测失败，可以直接退出外层循环，因为在往下匹配面容，也是失败的
                    error!("活体检测失败，真实概率: {:.2}%", real_prob * 100.0);
                    break 'face;
                }
            }

            let score = {
                recognizer
                    .match_(
                        &dst_feature,
                        &cur_feature,
                        FaceRecognizerSF_DisType::FR_COSINE.into(),
                    )
                    .map_err(|e| format!("特征匹配失败: {}", e))?
            };

            if score * 100.0 >= json_data.threshold.into() {
                // 匹配成功，次数+1
                success_count += 1;
                if success_count >= MAX_SUCCESS {
                    // 大于3次，算面容匹配成功
                    let user_name = if account_type == "local" {
                        format!(".\\{}", user_name)
                    } else {
                        user_name
                    };

                    if let Err(e) = unlock(user_name, user_pwd) {
                        return Err(format!("调用解锁函数失败：{}", e));
                    } else {
                        if let Err(e) = insert_unlock_log(&conn, id, true, "") {
                            warn!("插入解锁日志失败：{}", e);
                        };
                        info!("面容匹配成功，发送用户名密码");
                        return Ok(true);
                    }
                }
            } else {
                success_count = 0;
                fail_count += 1;
                if fail_count >= MAX_FAIL {
                    break;
                }
            }

            sleep(Duration::from_millis(50));
        }
    }

    // 发个假的用户名密码，通知用户解锁失败
    if let Err(e) = unlock(String::from("null"), String::from("null")) {
        return Err(format!("调用解锁函数失败：{}", e));
    }

    let mut save_file = true;
    let path = get_global_log_path().join("block");
    if !path.exists() {
        if let Err(e) = std::fs::create_dir_all(path.clone()) {
            save_file = false;
            error!("创建 faces 文件夹失败: {}", e);
        }
    }

    let img_name = format!("{}.faceimg", uuid::Uuid::new_v4());
    let binding = path.join(&img_name);
    let img_path = binding.to_str().unwrap_or("C:\\faceimg.faceimg");

    if save_file {
        // 保存最后一帧图片
        if let Err(e) = save_mat_as_faceimg(&frame, img_path) {
            error!("保存最后一帧图片失败: {}", e);
            save_file = false;
        }
    }

    if let Err(e) = insert_unlock_log(&conn, -1, false, if save_file { &img_name } else { "" }) {
        warn!("插入解锁日志失败：{}", e);
    };
    warn!("面容匹配失败");
    // 匹配失败，次数+1
    let now_count = MATCH_FAIL_COUNT.load(Ordering::SeqCst);
    MATCH_FAIL_COUNT.store(now_count + 1, Ordering::SeqCst);

    Ok(false)
}

fn open_camera(backend: Option<CameraBackend>, camear_index: i32) -> Result<VideoCapture, String> {
    // 尝试的列表
    let backends_to_try = match backend {
        // 指定了：只尝试该后端
        Some(backend) => vec![backend],
        // 未指定：尝试所有常用后端
        None => vec![
            CameraBackend::DShow,
            CameraBackend::Any,
            CameraBackend::MSMF,
            CameraBackend::VFW,
        ],
    };

    // 循环尝试不同后端
    for (idx, backend_inner) in backends_to_try.iter().enumerate() {
        match try_open_camera_with_backend(*backend_inner, camear_index) {
            Ok(cam) => {
                // 成功打开
                let msg = if backend.is_some() {
                    format!("使用指定后端 {:?} 成功打开摄像头", backend)
                } else {
                    format!("尝试第{}个后端 {:?} 成功打开摄像头", idx + 1, backend)
                };
                info!("{}", msg);
                return Ok(cam);
            }
            Err(e) => {
                // 处理失败情况
                if backend.is_some() {
                    // 指定了后端但失败：直接返回错误
                    return Err(format!("使用指定后端 {:?} 打开摄像头失败: {}", backend, e));
                } else {
                    // 未指定后端：打印尝试失败日志，继续尝试下一个
                    warn!("尝试后端 {:?} 失败: {}", backend, e);
                    continue;
                }
            }
        }
    }

    // 所有后端都尝试失败
    Err("所有摄像头后端均尝试失败，请检查设备是否连接/被占用/有权限".to_string())
}

// 从摄像头中读取视频帧
fn read_mat_from_camera(camera: &mut VideoCapture) -> Result<Mat, String> {
    let mut frame = Mat::default();

    camera
        .read(&mut frame)
        .map_err(|e| format!("摄像头读取失败: {}", e))?;

    if frame.empty() {
        return Err(String::from("抓取到空帧"));
    }

    Ok(frame)
}

// 使用指定后端尝试打开摄像头并验证读取帧
fn try_open_camera_with_backend(
    backend: CameraBackend,
    camear_index: i32,
) -> Result<VideoCapture, Box<dyn std::error::Error>> {
    let mut cam = VideoCapture::new(camear_index, backend.into())?;

    if !cam.is_opened()? {
        return Err(format!("后端 {:?} 打开摄像头后状态为未激活", backend).into());
    }

    // 激活摄像头
    let mut frame = Mat::default();
    let read_result = cam.read(&mut frame);

    match read_result {
        Ok(_) => {
            if frame.empty() {
                return Err(format!("后端 {:?} 读取到空帧", backend).into());
            }
        }
        Err(e) => {
            return Err(format!("后端 {:?} 读取帧失败: {}", backend, e).into());
        }
    }

    Ok(cam)
}

// 从文件加载人脸数据
fn load_face_data(path: &PathBuf) -> Result<FaceDescriptor, Box<dyn std::error::Error>> {
    let mut file = std::fs::File::open(path)?;
    let mut buffer = Vec::new();
    file.read_to_end(&mut buffer)?;
    let decoded: FaceDescriptor = bincode::deserialize(&buffer)?;
    Ok(decoded)
}

// 提取特征点
fn get_feature(
    img: &Mat,
    face_detection_threshold: f32,
    detector: &mut Ptr<FaceDetectorYN>,
    recognizer: &mut Ptr<FaceRecognizerSF>,
) -> Result<(Mat, Mat), String> {
    let faces = {
        let mut faces = Mat::default();
        detector
            .set_input_size(img.size().map_err(|e| format!("获取Mat尺寸失败: {}", e))?)
            .map_err(|e| format!("设置输入尺寸失败: {}", e))?;
        detector
            .set_score_threshold(face_detection_threshold)
            .map_err(|e| format!("设置分数阈值失败: {}", e))?;
        detector
            .detect(img, &mut faces)
            .map_err(|e| format!("OpenCV 检测失败: {}", e))?;
        faces
    };

    if faces.rows() > 0 {
        let mut aligned = Mat::default();
        let mut feature = Mat::default();
        // 人脸对齐与裁剪
        recognizer
            .align_crop(img, &faces.row(0).unwrap(), &mut aligned)
            .map_err(|e| format!("人脸对齐失败: {}", e))?;
        // 提取特征
        recognizer
            .feature(&aligned, &mut feature)
            .map_err(|e| format!("特征提取失败: {}", e))?;

        Ok((aligned.clone(), feature.clone()))
    } else {
        Err("未检测到人脸".into())
    }
}

fn insert_unlock_log(
    conn: &r2d2_sqlite::rusqlite::Connection,
    face_id: i32,
    is_unlock: bool,
    img_path: &str
) -> Result<(), String> {
    let mut insert_stmt = conn
        .prepare("INSERT INTO unlock_log (face_id, is_unlock, block_img) VALUES (?1, ?2, ?3)")
        .map_err(|e| format!("准备插入解锁日志语句失败：{:?}", e))?;

    // 插入数据
    insert_stmt
        .execute(r2d2_sqlite::rusqlite::params![
            face_id,
            if is_unlock { 1 } else { 0 },
            if img_path.is_empty() { None } else { Some(img_path) }
        ])
        .map_err(|e| format!("插入解锁日志失败：{:?}", e))?;
    Ok(())
}
