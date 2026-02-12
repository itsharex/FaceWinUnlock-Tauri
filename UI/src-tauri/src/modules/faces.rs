use std::{
    fs, io::{Read, Write}, path::PathBuf, thread::sleep, time::Duration
};

use crate::{utils::custom_result::CustomResult, APP_STATE, ROOT_DIR};
use base64::{engine::general_purpose, Engine};
use opencv::{
    core::{Mat, Point, Point2f, Rect, Scalar, Size, Vector},
    imgcodecs, imgproc,
    objdetect::FaceRecognizerSF_DisType,
    prelude::*,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;

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

struct CaptureResponse {
    display_base64: String, // 带框的
    raw_base64: String,     // 不带框的（仅缩放）
}

// 从图片中检测人脸
#[tauri::command]
pub fn check_face_from_img(
    img_path: String,
    face_detection_threshold: f32,
) -> Result<CustomResult, CustomResult> {
    // 从fs读取图片
    // opencv不支持中文，搞了半个小时 ...
    let bytes = std::fs::read(&img_path)
        .map_err(|e| CustomResult::error(Some(format!("图片读取失败: {}", e)), None))?;
    let v = Vector::<u8>::from_iter(bytes);
    let src = imgcodecs::imdecode(&v, imgcodecs::IMREAD_COLOR)
        .map_err(|e| CustomResult::error(Some(format!("OpenCV 解码失败: {}", e)), None))?;

    if src.empty() {
        return Err(CustomResult::error(
            Some(String::from("图片读取失败")),
            None,
        ));
    }

    let result = detect_and_format(src, face_detection_threshold)
        .map_err(|e| CustomResult::error(Some(format!("OpenCV 检测失败: {}", e)), None))?;

    Ok(CustomResult::success(
        None,
        Some(json!({
            "display_base64": result.display_base64,
            "raw_base64": result.raw_base64
        })),
    ))
}

// 从摄像头中检测人脸
#[tauri::command]
pub fn check_face_from_camera(face_detection_threshold: f32) -> Result<CustomResult, CustomResult> {
    let frame = read_mat_from_camera()
        .map_err(|e| CustomResult::error(Some(format!("摄像头读取失败: {}", e)), None))?;

        
    let result = detect_and_format(frame, face_detection_threshold)
        .map_err(|e| CustomResult::error(Some(format!("OpenCV 检测失败: {}", e)), None))?;

    Ok(CustomResult::success(
        None,
        Some(json!({
            "display_base64": result.display_base64,
            "raw_base64": result.raw_base64
        })),
    ))
}

// 一致性验证
#[tauri::command]
pub async fn verify_face(
    reference_base64: String,
    face_detection_threshold: f32,
    liveness_enabled: bool,
    liveness_threshold: f32,
    face_aligned_type: String,
) -> Result<CustomResult, CustomResult> {
    let frame = read_mat_from_camera()
        .map_err(|e| CustomResult::error(Some(format!("摄像头读取失败: {}", e)), None))?;
    let mut resized_mat_v = frame.clone();
    if let Ok(new_mat) = resize_mat(&frame, 800.0) {
        resized_mat_v = new_mat;
    }

    // 获取特征点，并获取人脸，对人脸进行活体检测
    // 如果对整个图片进行活体检测，误判机率很高
    let result = get_feature(&resized_mat_v, face_detection_threshold);
    if let Err(e) = &result {
        if e.contains("未检测到人脸") {
            return Ok(CustomResult::success(
                None,
                Some(json!(
                    {
                        "success": false,
                        "message": "未检测到人脸",
                        "score": 0,
                        "display_base64": mat_to_base64(&resized_mat_v)
                    }
                )),
            ))
        } else {
            CustomResult::error(Some(format!("特征提取失败: {}", e)), None);
        }
    }
    let (cur_aligned, cur_feature, face_range) = result.unwrap();
    
    if liveness_enabled {
        let mut app_state = APP_STATE
            .lock()
            .map_err(|e| CustomResult::error(Some(format!("获取app状态失败 {}", e)), None))?;
        // 开启了活体检测
        if app_state.liveness.is_none() {
            return Err(CustomResult::error(
                Some(String::from("活体检测模型未初始化")),
                None,
            ));
        }
        let liveness_net = &mut app_state.liveness.as_mut().unwrap().inner;

        // 图像预处理
        let face_data = face_range.at_row::<f32>(0).unwrap();
        let aligned_face = if face_aligned_type == "default" {
            align_face(&frame, face_data).map_err(|e| CustomResult::error(Some(format!("对齐人脸失败: {}", e)), None))?
        } else {
            cur_aligned
        };

        let blob = opencv::dnn::blob_from_image(&aligned_face, 1.0/255.0, Size::new(128, 128), Scalar::all(0.0), true, false, opencv::core::CV_32F)
        .map_err(|e| CustomResult::error(Some(format!("图像预处理失败: {}", e)), None))?;
        liveness_net.set_input(&blob, "", 1.0, Scalar::default())
        .map_err(|e| CustomResult::error(Some(format!("设置输入失败: {}", e)), None))?;
        let mut output_blobs = Vector::<Mat>::new();
        liveness_net.forward(&mut output_blobs, &liveness_net.get_unconnected_out_layers_names().map_err(|e| CustomResult::error(Some(format!("获取输出层失败: {}", e)), None))?)
        .map_err(|e| CustomResult::error(Some(format!("执行推理失败: {}", e)), None))?; 

        let mut is_real = false;
        let mut real_score = 0.0;

        if !output_blobs.is_empty() {
            let p = liveness_threshold.max(1e-6).min(1.0 - 1e-6);
            let logit_threshold = (p / (1.0 - p)).ln();

            let output = output_blobs.get(0).map_err(|_| CustomResult::error(Some(String::from("无输出")), None))?;
            let logits = output.at_row::<f32>(0).map_err(|e| CustomResult::error(Some(format!("获取输出行失败: {:?}", e)), None))?;
            real_score = logits[0] - logits[1];
            is_real = real_score >= logit_threshold;
        }
        
        if !is_real {
            return Ok(CustomResult::success(
                None,
                Some(json!(
                    {
                        "success": false,
                        "message": format!("活体检测未通过，概率 {:.2}%", real_score * 100.0),
                        "score": 0,
                        "display_base64": mat_to_base64(&resized_mat_v)
                    }
                )),
            ))
        }
    }
    // 解码图片
    let ref_bytes = general_purpose::STANDARD
        .decode(reference_base64)
        .map_err(|e| CustomResult::error(Some(format!("图片解码失败: {}", e)), None))?;
    let v = Vector::<u8>::from_iter(ref_bytes);
    let ref_img = imgcodecs::imdecode(&v, opencv::imgcodecs::IMREAD_COLOR)
        .map_err(|e| CustomResult::error(Some(format!("从bse64读取图片失败: {}", e)), None))?;

    let (_ref_aligned, ref_feature, _) = get_feature(&ref_img, face_detection_threshold)
        .map_err(|e| CustomResult::error(Some(format!("特征提取失败: {}", e)), None))?;
    

    let app_state = APP_STATE
        .lock()
        .map_err(|e| CustomResult::error(Some(format!("获取app状态失败 {}", e)), None))?;
    let Some(recognizer) = app_state.recognizer.as_ref() else {
        return Err(CustomResult::error(
            Some(String::from("人脸识别模型未初始化")),
            None,
        ));
    };

    let score = recognizer
        .inner
        .match_(
            &ref_feature,
            &cur_feature,
            FaceRecognizerSF_DisType::FR_COSINE.into(),
        )
        .map_err(|e| CustomResult::error(Some(format!("特征匹配失败: {}", e)), None))?;

    Ok(CustomResult::success(
        None,
        Some(json!(
            {
                "success": true,
                "message": "",
                "score": score,
                "display_base64": mat_to_base64(&resized_mat_v)
            }
        )),
    ))
}

// 保存特征到文件
#[tauri::command]
pub fn save_face_registration(
    name: String,
    reference_base64: String,
    face_detection_threshold: f32,
) -> Result<CustomResult, CustomResult> {
    // 获取软件数据目录并创建 faces 文件夹
    let path = ROOT_DIR.join("faces");

    if !path.exists() {
        std::fs::create_dir_all(&path).map_err(|e| {
            CustomResult::error(Some(format!("创建 faces 文件夹失败: {}", e)), None)
        })?;
    }

    // 解码图片
    let ref_bytes = general_purpose::STANDARD
        .decode(reference_base64)
        .map_err(|e| CustomResult::error(Some(format!("图片解码失败: {}", e)), None))?;
    let v = Vector::<u8>::from_iter(ref_bytes);
    let ref_img = imgcodecs::imdecode(&v, opencv::imgcodecs::IMREAD_COLOR)
        .map_err(|e| CustomResult::error(Some(format!("从bse64读取图片失败: {}", e)), None))?;

    let (_ref_aligned, ref_feature, _) = get_feature(&ref_img, face_detection_threshold)
        .map_err(|e| CustomResult::error(Some(format!("特征提取失败: {}", e)), None))?;

    let descriptor = FaceDescriptor::from_mat(&name, &ref_feature)
        .map_err(|e| CustomResult::error(Some(format!("特征描述失败: {}", e)), None))?;

    let base_name = Uuid::new_v4();

    // 保存特征
    let feature_name = format!("{}.face", base_name);
    let mut feature_path = path.clone();
    feature_path.push(feature_name);
    save_face_data(&feature_path, &descriptor)
        .map_err(|e| CustomResult::error(Some(format!("保存特征数据失败: {}", e)), None))?;

    // 保存图片
    let file_name = format!("{}.faceimg", base_name);
    let mut file_path = path.clone();
    file_path.push(file_name);
    let resize_mat: Mat = resize_mat(&ref_img, 800.0)
        .map_err(|e| CustomResult::error(Some(format!("图片缩放失败: {}", e)), None))?;

    let mut buf = Vector::<u8>::new();
    imgcodecs::imencode(".jpg", &resize_mat, &mut buf, &Vector::new()).unwrap();
    fs::write(file_path, buf).map_err(|e| {
        // 图片保存失败删除面容特征
        if let Err(err) = fs::remove_file(feature_path.clone()) {
            CustomResult::error(
                Some(format!(
                    "特征文件删除失败: {} 文件地址：{:?}",
                    err, feature_path
                )),
                None,
            )
        } else {
            CustomResult::error(Some(format!("图片保存失败: {}", e)), None)
        }
    })?;

    Ok(CustomResult::success(
        None,
        Some(json!({"file_name": base_name})),
    ))
}

/// 提取特征点
/// return (裁切后的图片, 特征点)
pub fn get_feature(img: &Mat, face_detection_threshold: f32) -> Result<(Mat, Mat, Mat), String> {
    let mut app_state = APP_STATE
        .lock()
        .map_err(|e| format!("获取app状态失败 {}", e))?;

    if app_state.detector.is_none() {
        return Err(String::from("人脸检测模型未初始化"));
    }
    if app_state.recognizer.is_none() {
        return Err(String::from("人脸识别模型未初始化"));
    }

    let faces = {
        let detector = app_state.detector.as_mut().unwrap();
        let mut faces = Mat::default();
        detector
            .inner
            .set_input_size(img.size().map_err(|e| format!("获取Mat尺寸失败: {}", e))?)
            .map_err(|e| format!("设置输入尺寸失败: {}", e))?;
        detector
            .inner
            .set_score_threshold(face_detection_threshold)
            .map_err(|e| format!("设置分数阈值失败: {}", e))?;
        detector
            .inner
            .detect(img, &mut faces)
            .map_err(|e| format!("OpenCV 检测失败: {}", e))?;
        faces
    };

    if faces.rows() > 0 {
        let mut aligned = Mat::default();
        let mut feature = Mat::default();

        let recognizer = app_state.recognizer.as_mut().unwrap();
        // 人脸对齐与裁剪
        recognizer
            .inner
            .align_crop(img, &faces.row(0).unwrap(), &mut aligned)
            .map_err(|e| format!("人脸对齐失败: {}", e))?;
        // 提取特征
        recognizer
            .inner
            .feature(&aligned, &mut feature)
            .map_err(|e| format!("特征提取失败: {}", e))?;

        Ok((aligned.clone(), feature.clone(), faces.clone()))
    } else {
        Err("未检测到人脸".into())
    }
}

// 从摄像头中读取视频帧
pub fn read_mat_from_camera() -> Result<Mat, String> {
    // 此处在 proc中，face_recog_type == "operation" 时，如果系统进入睡眠状态
    // 这里会变成死锁，而Win + L锁屏就不会，并且按延迟时间的解锁，即便进入睡眠状态
    // 也不会变成死锁，具体原因不明，真让人头大...
    // 所以这里改成 try_lock，不堵塞主线程了
    let mut app_state = None;
    for _ in 0..3 {
        if let Ok(inner) = APP_STATE.try_lock() {
            app_state = Some(inner);
            break;
        }
        sleep(Duration::from_millis(100));
    }
    
    // 重试3次还拿不到，再返回错误
    let mut app_state = app_state.ok_or_else(|| {
        format!("获取app状态失败（锁被占用超过300ms）")
    })?;

    // 如果摄像头没打开
    if app_state.camera.is_none() {
        return Err(String::from("请先打开摄像头"));
    }

    let cam = app_state.camera.as_mut().unwrap();
    let mut frame = Mat::default();

    cam.inner
        .read(&mut frame)
        .map_err(|e| format!("摄像头读取失败: {}", e))?;

    if frame.empty() {
        return Err(String::from("抓取到空帧"));
    }

    Ok(frame)
}

// 等比例缩放Mat
fn resize_mat(src: &Mat, max_dim: f32) -> Result<Mat, String> {
    let size = src.size().map_err(|e| e.to_string())?;
    let scale = (max_dim / (size.width.max(size.height) as f32)).min(1.0);

    let mut resize_mat = Mat::default();
    if scale < 1.0 {
        let new_size = Size::new(
            (size.width as f32 * scale) as i32,
            (size.height as f32 * scale) as i32,
        );
        imgproc::resize(
            &src,
            &mut resize_mat,
            new_size,
            0.0,
            0.0,
            imgproc::INTER_AREA,
        )
        .ok();
    } else {
        resize_mat = src.clone();
    }

    Ok(resize_mat)
}

// 处理人脸特征点
fn detect_and_format(src: Mat, face_detection_threshold: f32) -> Result<CaptureResponse, String> {
    let mut app_state = APP_STATE
        .lock()
        .map_err(|e| format!("获取app状态失败 {}", e))?;

    let Some(detector) = app_state.detector.as_mut() else {
        return Err(String::from("人脸检测模型未初始化"));
    };

    // 等比例缩放
    let raw_mat = resize_mat(&src, 800.0)?;

    // 检测
    let mut display_mat = raw_mat.clone(); // 用于显示的副本
    let mut faces = Mat::default();
    detector
        .inner
        .set_input_size(
            display_mat
                .size()
                .map_err(|e| format!("获取Mat尺寸失败: {}", e))?,
        )
        .map_err(|e| format!("设置输入尺寸失败: {}", e))?;
    detector
        .inner
        .set_score_threshold(face_detection_threshold)
        .map_err(|e| format!("设置分数阈值失败: {}", e))?;
    detector
        .inner
        .detect(&display_mat, &mut faces)
        .map_err(|e| format!("OpenCV 检测失败: {}", e))?;

    if faces.rows() > 0 {
        let x = *faces
            .at_2d::<f32>(0, 0)
            .map_err(|e| format!("图片坐标获取失败: {}", e))?;
        let y = *faces
            .at_2d::<f32>(0, 1)
            .map_err(|e| format!("图片坐标获取失败: {}", e))?;
        let w = *faces
            .at_2d::<f32>(0, 2)
            .map_err(|e| format!("图片坐标获取失败: {}", e))?;
        let h = *faces
            .at_2d::<f32>(0, 3)
            .map_err(|e| format!("图片坐标获取失败: {}", e))?;

        let color = Scalar::new(255.0, 242.0, 0.0, 0.0);
        imgproc::rectangle(
            &mut display_mat,
            Rect::new(x as i32, y as i32, w as i32, h as i32),
            color,
            2,
            imgproc::LINE_8,
            0,
        )
        .map_err(|e| format!("图片绘制失败: {}", e))?;

        // 绘制五官
        for i in (4..14).step_by(2) {
            // 五官不影响检测结果，所以绘制失败可以忽略
            if let (Ok(px), Ok(py)) = (faces.at_2d::<f32>(0, i), faces.at_2d::<f32>(0, i + 1)) {
                imgproc::circle(
                    &mut display_mat,
                    Point::new(*px as i32, *py as i32),
                    4,
                    Scalar::new(0.0, 255.0, 0.0, 0.0), // 绿色
                    -1,
                    imgproc::LINE_AA,
                    0,
                )
                .ok();
            }
        }

        Ok(CaptureResponse {
            display_base64: mat_to_base64(&display_mat),
            raw_base64: mat_to_base64(&raw_mat),
        })
    } else {
        Ok(CaptureResponse {
            display_base64: String::from("未检测到人脸"),
            raw_base64: mat_to_base64(&raw_mat),
        })
    }
}

fn mat_to_base64(mat: &Mat) -> String {
    let mut buf = Vector::<u8>::new();
    imgcodecs::imencode(".jpg", mat, &mut buf, &Vector::new()).unwrap();
    format!(
        "data:image/jpeg;base64,{}",
        general_purpose::STANDARD.encode(buf.as_slice())
    )
}

// 保存人脸数据到文件
fn save_face_data(
    path: &std::path::PathBuf,
    data: &FaceDescriptor,
) -> Result<(), Box<dyn std::error::Error>> {
    let encoded: Vec<u8> = bincode::serialize(data)?;
    let mut file = std::fs::File::create(path)?;
    file.write_all(&encoded)?;
    Ok(())
}

// 从文件加载人脸数据
pub fn load_face_data(path: &PathBuf) -> Result<FaceDescriptor, Box<dyn std::error::Error>> {
    let mut file = std::fs::File::open(path)?;
    let mut buffer = Vec::new();
    file.read_to_end(&mut buffer)?;
    let decoded: FaceDescriptor = bincode::deserialize(&buffer)?;
    Ok(decoded)
}

// 手动裁剪人脸
fn align_face(frame: &Mat, face_data: &[f32]) -> opencv::Result<Mat> {
    // 获取关键点
    let left_eye = Point2f::new(face_data[4], face_data[5]);
    let right_eye = Point2f::new(face_data[6], face_data[7]);

    // 计算目标位置（将双眼对齐到 128x128 的固定位置）
    // 设定目标图像中眼睛的理想位置，这决定了人脸在框内的占比
    let target_w = 128.0;
    let target_h = 128.0;
    let eye_y_position = 0.35; // 眼睛位于上方 35% 处
    let eye_x_distance = 0.30; // 眼睛距离中心两侧的距离

    let desired_left_eye = Point2f::new(target_w * (0.5 - eye_x_distance), target_h * eye_y_position);
    let desired_right_eye = Point2f::new(target_w * (0.5 + eye_x_distance), target_h * eye_y_position);

    // 计算相似变换矩阵 (Similarity Transform)
    // 根据两眼中心、角度、距离进行缩放和平移
    let d_x = right_eye.x - left_eye.x;
    let d_y = right_eye.y - left_eye.y;
    let dist = (d_x.powi(2) + d_y.powi(2)).sqrt();
    let angle = (d_y as f64).atan2(d_x as f64) * 180.0 / std::f64::consts::PI;

    let desired_dist = (desired_right_eye.x - desired_left_eye.x) as f64;
    let scale = desired_dist / dist as f64;

    let center = Point2f::new((left_eye.x + right_eye.x) / 2.0, (left_eye.y + right_eye.y) / 2.0);

    // 获取基础旋转缩放矩阵 (2x3 矩阵)
    let mut trans_mat = imgproc::get_rotation_matrix_2d(center, angle, scale)?;

    // 修正平移分量 (Column 2)
    let target_center_x = target_w * 0.5;
    let target_center_y = target_h * eye_y_position;

    // 根据仿射变换公式：dst = M * src
    // M_02 = target_x - (M_00 * src_x + M_01 * src_y)
    // M_12 = target_y - (M_10 * src_x + M_11 * src_y)
    let m = trans_mat.at_row::<f64>(0)?; // 获取第一行数据指针
    let m00 = m[0];
    let m01 = m[1];
    let m10 = trans_mat.at_row::<f64>(1)?[0];
    let m11 = trans_mat.at_row::<f64>(1)?[1];

    let tx = target_center_x as f64 - (m00 * center.x as f64 + m01 * center.y as f64);
    let ty = target_center_y as f64 - (m10 * center.x as f64 + m11 * center.y as f64);

    // 安全地更新平移向量
    *trans_mat.at_2d_mut::<f64>(0, 2)? = tx;
    *trans_mat.at_2d_mut::<f64>(1, 2)? = ty;

    // 执行变换
    let mut aligned_face = Mat::default();
    imgproc::warp_affine(
        &frame, 
        &mut aligned_face, 
        &trans_mat, 
        Size::new(128, 128), 
        imgproc::INTER_LINEAR, 
        opencv::core::BORDER_CONSTANT, 
        Scalar::default()
    )?;

    Ok(aligned_face)
}