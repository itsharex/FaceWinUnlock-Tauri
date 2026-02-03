use std::{os::windows::process::CommandExt, process::Command};

use crate::{
    modules::options::{write_to_registry, RegistryItem},
    utils::custom_result::CustomResult,
    OpenCVResource, APP_STATE, GLOBAL_TRAY, ROOT_DIR,
};
use opencv::{
    core::{Mat, MatTraitConst, Size},
    objdetect::{FaceDetectorYN, FaceRecognizerSF},
    videoio::{self, VideoCapture, VideoCaptureTrait, VideoCaptureTraitConst},
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tauri::{AppHandle, Manager};
use tauri_plugin_log::log::{error, info, warn};
use windows::{
    core::{BSTR, HSTRING, PWSTR},
    Win32::{
        Foundation::{E_UNEXPECTED, HWND},
        Media::{
            DirectShow::ICreateDevEnum,
            MediaFoundation::{CLSID_SystemDeviceEnum, CLSID_VideoInputDeviceCategory},
        },
        System::{
            Com::{
                CoCreateInstance, CoInitializeEx, CoUninitialize, IEnumMoniker,
                StructuredStorage::IPropertyBag, CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED,
            },
            RemoteDesktop::WTSUnRegisterSessionNotification,
            Shutdown::LockWorkStation,
            Variant::{VariantClear, VARIANT},
            WindowsProgramming::GetUserNameW,
        },
    },
};

use super::pipe::Client;

#[derive(Debug, Clone, Serialize)]
struct ValidCameraInfo {
    camera_name: String,
    capture_index: String,
    is_valid: bool,
}

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

// 获取当前用户名
#[tauri::command]
pub fn get_now_username() -> Result<CustomResult, CustomResult> {
    // buffer大小，256应该够了
    let mut buffer = [0u16; 256];
    let mut size = buffer.len() as u32;
    unsafe {
        let succuess = GetUserNameW(Some(PWSTR(buffer.as_mut_ptr())), &mut size);
        if succuess.is_err() {
            return Err(CustomResult::error(
                Some(format!("获取用户名失败: {:?}", succuess.err())),
                None,
            ));
        }

        let name = String::from_utf16_lossy(&buffer[..size as usize - 1]);
        return Ok(CustomResult::success(None, Some(json!({"username": name}))));
    }
}

// 测试 WinLogon 是否加载成功
#[tauri::command]
pub fn test_win_logon(user_name: String, password: String) -> Result<CustomResult, CustomResult> {
    // 锁定屏幕
    unsafe {
        let succuess = LockWorkStation();
        if succuess.is_err() {
            return Err(CustomResult::error(
                Some(format!("锁定屏幕失败: {:?}", succuess.err())),
                None,
            ));
        }

        // 等待5秒
        std::thread::sleep(std::time::Duration::from_secs(5));
        // 解锁
        unlock(user_name, password)
            .map_err(|e| CustomResult::error(Some(format!("解锁屏幕失败: {:?}", e)), None))?;

        // 连接成功，允许连接
        write_to_registry(vec![RegistryItem {
            key: String::from("CONNECT_TO_PIPE"),
            value: String::from("1"),
        }])?;
    }
    return Ok(CustomResult::success(None, None));
}

// 初始化模型
#[tauri::command]
pub fn init_model() -> Result<CustomResult, CustomResult> {
    // 加载模型
    let resource_path = ROOT_DIR
        .join("resources")
        .join("face_detection_yunet_2023mar.onnx");

    // 这个不用检查文件是否存在，不存在opencv会报错
    let _ = FaceDetectorYN::create(
        resource_path.to_str().unwrap_or(""),
        "",
        Size::new(320, 320), // 初始尺寸，后面会动态更新
        0.9,
        0.3,
        5000,
        0,
        0,
    )
    .map_err(|e| CustomResult::error(Some(format!("初始化检测器模型失败: {:?}", e)), None))?;

    let resource_path = ROOT_DIR
        .join("resources")
        .join("face_recognition_sface_2021dec.onnx");
    let _ = FaceRecognizerSF::create(resource_path.to_str().unwrap_or(""), "", 0, 0)
        .map_err(|e| CustomResult::error(Some(format!("初始化识别器模型失败: {:?}", e)), None))?;

    // 加载活体检测模型
    let _ = opencv::dnn::read_net_from_onnx(ROOT_DIR.join("resources").join("face_liveness.onnx").to_str().unwrap())
            .map_err(|e| CustomResult::error(Some(format!("初始化活体检测模型失败: {:?}", e)), None))?;

    Ok(CustomResult::success(None, None))
}

// 获取windows所有摄像头
#[tauri::command]
pub fn get_camera() -> Result<CustomResult, CustomResult> {
    // 初始化COM
    let com_init_result = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
    if com_init_result.is_err() {
        return Err(CustomResult::error(
            Some(String::from("初始化Com失败")),
            None,
        ));
    }

    let com_operation_result = get_windows_video_devices();
    // 卸载Com
    unsafe { CoUninitialize() };

    if let Err(e) = com_operation_result {
        return Err(CustomResult::error(
            Some(format!("获取系统摄像头失败 {}", e)),
            None,
        ));
    }

    let video_devices = com_operation_result.unwrap();
    if video_devices.is_empty() {
        return Err(CustomResult::error(
            Some(String::from("未检测到系统视频设备（摄像头）")),
            None,
        ));
    }

    // 判断摄像头可用性
    let mut valid_cameras = Vec::new();
    for (camera_name, index) in video_devices {
        match is_camera_index_valid(index) {
            Ok(is_valid) => {
                valid_cameras.push(ValidCameraInfo {
                    camera_name,
                    capture_index: index.to_string(),
                    is_valid: is_valid,
                });
            }
            _ => {}
        }
    }

    Ok(CustomResult::success(None, Some(json!(valid_cameras))))
}

// 打开摄像头
#[tauri::command]
pub fn open_camera(
    backend: Option<CameraBackend>,
    camear_index: i32,
) -> Result<CustomResult, CustomResult> {
    let mut app_state = APP_STATE
        .lock()
        .map_err(|e| CustomResult::error(Some(format!("获取app状态失败 {}", e)), None))?;

    // 如果摄像头已打开，直接返回成功
    if app_state.camera.is_some() {
        return Ok(CustomResult::success(None, None));
    }

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
                app_state.camera = Some(OpenCVResource { inner: cam });
                let msg = if backend.is_some() {
                    format!("使用指定后端 {:?} 成功打开摄像头", backend)
                } else {
                    format!("尝试第{}个后端 {:?} 成功打开摄像头", idx + 1, backend)
                };
                info!("{}", msg);
                return Ok(CustomResult::success(None, None));
            }
            Err(e) => {
                // 处理失败情况
                if backend.is_some() {
                    // 指定了后端但失败：直接返回错误
                    return Err(CustomResult::error(
                        Some(format!("使用指定后端 {:?} 打开摄像头失败: {}", backend, e)),
                        None,
                    ));
                } else {
                    // 未指定后端：打印尝试失败日志，继续尝试下一个
                    warn!("尝试后端 {:?} 失败: {}", backend, e);
                    continue;
                }
            }
        }
    }

    // 所有后端都尝试失败
    Err(CustomResult::error(
        Some("所有摄像头后端均尝试失败，请检查设备是否连接/被占用/有权限".to_string()),
        None,
    ))
}

// 关闭摄像头
#[tauri::command]
pub fn stop_camera() -> Result<CustomResult, CustomResult> {
    let mut app_state = APP_STATE
        .lock()
        .map_err(|e| CustomResult::error(Some(format!("获取app状态失败 {}", e)), None))?;
    app_state.camera = None;
    Ok(CustomResult::success(None, None))
}

// 打开指定目录用资源管理器
#[tauri::command]
pub fn open_directory(path: String) -> Result<CustomResult, CustomResult> {
    let path = std::path::Path::new(&path);
    if !path.exists() {
        return Err(CustomResult::error(
            Some(format!("路径不存在 {}", path.display())),
            None,
        ));
    }

    std::process::Command::new("explorer")
        .arg(path)
        .status()
        .map_err(|e| {
            CustomResult::error(
                Some(format!(
                    "打开文件夹失败：{}<br>请手动打开文件夹：{:?}",
                    e,
                    path.to_str()
                )),
                None,
            )
        })?;

    Ok(CustomResult::success(None, None))
}

// 自启代码由 Google Gemini 3 生成
// 我写不了出来了，注册表不管用 哭**
const CREATE_NO_WINDOW: u32 = 0x08000000;
/// 通用计划任务创建函数
/// 参数说明：
/// - path: 程序相对路径（如 "Unlock.exe"）
/// - task_name: 任务名称
/// - is_server: 是否为无GUI（SYSTEM账户）模式
/// - silent: 是否静默运行
/// - run_on_system_start: 是否系统启动就运行（而非登录后），该参数为true时is_server强制为true
/// - run_immediately: 是否创建后立即运行任务
#[tauri::command]
pub fn add_scheduled_task(
    path: String,
    task_name: String,
    is_server: bool,
    silent: bool,
    run_on_system_start: bool,
    run_immediately: bool,
) -> Result<CustomResult, CustomResult> {
    // 强制约束：系统启动运行时，is_server必须为true
    if run_on_system_start && !is_server {
        return Err(CustomResult::error(
            Some(String::from("系统启动运行时，is_server必须为true")),
            None,
        ));
    }

    // 路径解析
    let binding = ROOT_DIR.join(path);
    let exe_path = binding
        .to_str()
        .ok_or_else(|| CustomResult::error(Some(String::from("程序路径解析失败")), None))?;

    // 构建任务运行命令
    let task_run = if silent {
        // 只给exe路径加引号，--silent参数在引号外
        quote_exe_path_with_args(exe_path, Some("--silent"))
    } else {
        quote_exe_path_with_args(exe_path, None)
    };

    // 构建 schtasks 创建参数
    let mut schtasks_args = vec![
        "/Create",
        "/TN",
        &task_name,
        "/TR",
        &task_run,
        "/SC",
        if run_on_system_start {
            "ONSTART"
        } else {
            "ONLOGON"
        },
        "/RL",
        "HIGHEST",
        "/F",
    ];

    // 追加差异参数
    if is_server {
        schtasks_args.extend(&["/RU", "SYSTEM", "/NP", "/DELAY", "0000:05"]);
    } else {
        schtasks_args.extend(&["/RU", "BUILTIN\\Users", "/IT", "/DELAY", "0000:10"]);
    }

    // 执行任务创建命令
    let create_output = Command::new("schtasks")
        .args(&schtasks_args)
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .map_err(|e| CustomResult::error(Some(format!("执行系统命令失败: {}", e)), None))?;

    if !create_output.status.success() {
        let err_msg = fix_gbk_encoding(&create_output.stderr);
        return Err(CustomResult::error(
            Some(format!("创建计划任务失败: {}", err_msg)),
            None,
        ));
    }

    // 构建 PowerShell 配置命令
    let ps_command = format!(
        r#"
        $task = Get-ScheduledTask -TaskName '{}' -ErrorAction Stop;
        $task.Settings.DisallowStartIfOnBatteries = $false;
        $task.Settings.StopIfGoingOnBatteries = $false;
        $task.Settings.AllowStartOnDemand = $true;
        $task.Settings.StartWhenAvailable = $true;
        $task.Settings.MultipleInstances = 'Parallel';
        $task.Settings.AllowHardTerminate = $true;
        $task.Settings.DisallowStartOnRemoteAppSession = $false;
        $task.Settings.IdleSettings.IdleDuration = 'PT0S';
        $task.Settings.IdleSettings.WaitTimeout = 'PT0S';
        $task.Settings.IdleSettings.StopOnIdleEnd = $false;
        $task.Settings.IdleSettings.RestartOnIdle = $false;
        $task.Settings.AllowInteractive = $true;
        $task.Principal.LogonType = '{}';
        $task.Principal.RunLevel = 'HighestAvailable';
        $task.Settings.RestartCount = 3;
        $task.Settings.RestartInterval = 'PT1M';
        Set-ScheduledTask -InputObject $task -ErrorAction Stop;
        Write-Host '任务配置更新成功';
        "#,
        task_name,
        if is_server {
            "ServiceAccount"
        } else {
            "InteractiveToken"
        }
    );

    // 执行 PowerShell 命令=
    let ps_args = vec![
        "-ExecutionPolicy",
        "Bypass",
        "-NoProfile",
        "-NonInteractive",
        "-Command",
        &ps_command,
    ];

    let ps_output = Command::new("powershell")
        .args(&ps_args)
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .map_err(|e| {
            CustomResult::error(Some(format!("PowerShell 修改任务设置失败: {}", e)), None)
        })?;

    if !ps_output.status.success() {
        let err_msg = fix_gbk_encoding(&ps_output.stderr);
        warn!("警告：修改任务高级设置失败，但基础任务已创建: {}", err_msg);
    }

    // 立即运行任务
    if run_immediately {
        match run_scheduled_task(&task_name) {
            Ok(_) => info!("任务创建成功并立即运行"),
            Err(e) => warn!("任务创建成功，但立即运行失败: {}", e),
        }
    }

    Ok(CustomResult::success(None, None))
}

// 禁用全用户自启动
#[tauri::command]
pub fn disable_scheduled_task(task_name: String) -> Result<CustomResult, CustomResult> {
    let output = Command::new("schtasks")
        .args(&["/Delete", "/TN", &task_name, "/F"])
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .map_err(|e| CustomResult::error(Some(format!("执行系统命令失败: {}", e)), None))?;

    if output.status.success() {
        Ok(CustomResult::success(None, None))
    } else {
        let err_msg = String::from_utf8_lossy(&output.stderr);
        // 如果任务本身不存在，删除会报错，这里可以根据需要判断是否视为成功
        Err(CustomResult::error(
            Some(format!("删除计划任务失败: {}", err_msg)),
            None,
        ))
    }
}

// 检查是否已开启全用户自启动
#[tauri::command]
pub fn check_scheduled_task(task_name: String) -> Result<CustomResult, CustomResult> {
    // /Query 检查任务是否存在
    let output = Command::new("schtasks")
        .args(&["/Query", "/TN", &task_name])
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .map_err(|e| CustomResult::error(Some(format!("查询系统命令失败: {}", e)), None))?;

    // 如果状态码为 0，说明任务存在
    let is_enabled = output.status.success();

    Ok(CustomResult::success(
        None,
        Some(json!({"enable": is_enabled})),
    ))
}

#[tauri::command]
pub fn check_process_running() -> Result<CustomResult, CustomResult> {
    let client = Client::new(HSTRING::from(r"\\.\pipe\MansonWindowsUnlockRustUnlock"));
    if client.is_err() {
        return Err(CustomResult::error(
            Some(format!("pipe错误: {}", client.err().unwrap())),
            None,
        ));
    }

    let client = client.unwrap();
    if let Err(e) = crate::utils::pipe::write(client.handle, String::from("hello server")) {
        return Err(CustomResult::error(
            Some(format!("向客户端写入数据失败: {:?}", e)),
            None,
        ));
    }

    Ok(CustomResult::success(None, None))
}

#[tauri::command]
pub fn delete_process_running() -> Result<CustomResult, CustomResult> {
    let client = Client::new(HSTRING::from(r"\\.\pipe\MansonWindowsUnlockRustUnlock"));
    if client.is_err() {
        return Err(CustomResult::error(
            Some(format!("pipe错误: {}", client.err().unwrap())),
            None,
        ));
    }

    let client = client.unwrap();
    if let Err(e) = crate::utils::pipe::write(client.handle, String::from("exit")) {
        return Err(CustomResult::error(
            Some(format!("向客户端写入数据失败: {:?}", e)),
            None,
        ));
    }

    Ok(CustomResult::success(None, None))
}

// 检查当前服务启动状态
#[tauri::command]
pub fn check_trigger_via_xml(task_name: &str) -> Result<String, String> {
    let output = Command::new("schtasks")
        .args(&["/Query", "/TN", task_name, "/XML"])
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .map_err(|e| format!("执行系统命令失败: {}", e))?;

    let xml_content = String::from_utf8_lossy(&output.stdout);

    if xml_content.contains("<LogonTrigger>") {
        Ok("OnLogon".to_string())
    } else if xml_content.contains("<BootTrigger>") {
        Ok("OnStart".to_string())
    } else {
        Ok("Unknown".to_string())
    }
}

// 关闭软件
#[tauri::command]
pub fn close_app(app_handle: AppHandle) -> Result<CustomResult, CustomResult> {
    let window = app_handle.get_webview_window("main").unwrap();
    let hwnd = window.hwnd().unwrap();
    unsafe {
        // 注销 WTS 通知
        let _ = WTSUnRegisterSessionNotification(HWND(hwnd.0));
    }

    // 关闭系统托盘
    let mut guard = GLOBAL_TRAY
        .lock()
        .map_err(|e| CustomResult::error(Some(format!("锁定托盘全局变量失败: {}", e)), None))?;
    if let Some(tray_any) = guard.as_mut() {
        tray_any
            .set_visible(false)
            .map_err(|e| CustomResult::error(Some(format!("隐藏托盘图标失败: {}", e)), None))?;
    }

    app_handle.exit(0);

    Ok(CustomResult::success(None, None))
}
#[tauri::command]
// 加载opencv模型
pub fn load_opencv_model() -> Result<(), String> {
    // 加载模型
    let mut app_state = APP_STATE
        .lock()
        .map_err(|e| format!("获取app状态失败 {}", e))?;

    if app_state.detector.is_none() {
        let resource_path = ROOT_DIR
            .join("resources")
            .join("face_detection_yunet_2023mar.onnx");

        // 这个不用检查文件是否存在，不存在opencv会报错
        let detector = FaceDetectorYN::create(
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

        app_state.detector = Some(OpenCVResource { inner: detector });
    }

    if app_state.recognizer.is_none() {
        let resource_path = ROOT_DIR
            .join("resources")
            .join("face_recognition_sface_2021dec.onnx");
        let recognizer = FaceRecognizerSF::create(resource_path.to_str().unwrap_or(""), "", 0, 0)
            .map_err(|e| format!("初始化识别器模型失败: {:?}", e))?;

        app_state.recognizer = Some(OpenCVResource { inner: recognizer });
    }

    if app_state.liveness.is_none() {
        let resource_path = ROOT_DIR
            .join("resources")
            .join("face_liveness.onnx");
        let liveness = opencv::dnn::read_net_from_onnx(resource_path.to_str().unwrap_or(""))
            .map_err(|e| format!("初始化活体检测模型失败: {:?}", e))?;

        app_state.liveness = Some(OpenCVResource { inner: liveness });
    }

    Ok(())
}

#[tauri::command]
// 卸载模型
pub fn unload_model() -> Result<(), String> {
    let mut app_state = APP_STATE
        .lock()
        .map_err(|e| format!("获取app状态失败 {}", e))?;

    if app_state.detector.is_some() {
        app_state.detector = None;
    }

    if app_state.recognizer.is_some() {
        app_state.recognizer = None;
    }

    if app_state.liveness.is_some() {
        app_state.liveness = None;
    }
    Ok(())
}

#[tauri::command]
// 获取uuid v4
pub fn get_uuid_v4() -> Result<String, String> {
    let uuid = uuid::Uuid::new_v4();
    Ok(uuid.to_string())
}

#[tauri::command]
// 获取软件的缓存目录
pub fn get_cache_dir() -> Result<String, String> {
    let app_data = std::env::var("ProgramData").unwrap_or_else(|_| "C:\\ProgramData".to_string());
    let webview_data_dir = format!("{}\\facewinunlock-tauri\\EBWebView", app_data);
    Ok(webview_data_dir)
}

#[tauri::command]
// 执行计划任务
pub fn run_scheduled_task(task_name: &str) -> Result<(), String> {
    // 执行 schtasks /Run 命令
    let run_output = Command::new("schtasks")
        .args(&["/Run", "/TN", task_name])
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .map_err(|e| format!("执行任务命令失败: {}", e))?;

    if !run_output.status.success() {
        let err_msg = fix_gbk_encoding(&run_output.stderr);
        return Err(format!("任务启动失败: {}", err_msg));
    }

    Ok(())
}

/// 处理带参数的路径，确保引号只包裹可执行文件路径，参数在外部
fn quote_exe_path_with_args(exe_path: &str, args: Option<&str>) -> String {
    // 只给可执行文件路径加引号（如果有空格），参数保持在引号外
    let quoted_exe = if exe_path.contains(' ') && !exe_path.starts_with('"') {
        format!("\"{}\"", exe_path)
    } else {
        exe_path.to_string()
    };

    // 拼接参数（如果有）
    match args {
        Some(arg) => format!("{} {}", quoted_exe, arg),
        None => quoted_exe,
    }
}

fn fix_gbk_encoding(bytes: &[u8]) -> String {
    let (s, _, _) = encoding_rs::GBK.decode(bytes);
    s.trim().to_string()
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
// 获取windows所有摄像头
fn get_windows_video_devices() -> windows::core::Result<Vec<(String, u32)>> {
    // 存放所有摄像头设备信息
    let mut devices = Vec::new();

    unsafe {
        // 创建ICreateDevEnum，用于获取摄像头设备，参考自微软官方文档
        // https://learn.microsoft.com/zh-cn/windows/win32/directshow/using-the-system-device-enumerator
        let dev_enum: ICreateDevEnum = CoCreateInstance(
            &CLSID_SystemDeviceEnum, // 系统设备枚举器的CLSID
            None,                    // 无聚合对象，传NULL
            CLSCTX_INPROC_SERVER,    // 进程内组件上下文
        )
        .map_err(|e| {
            error!("创建ICreateDevEnum失败");
            e
        })?;

        // 获取视频输入设备
        let mut enum_moniker: Option<IEnumMoniker> = None;
        dev_enum
            .CreateClassEnumerator(&CLSID_VideoInputDeviceCategory, &mut enum_moniker, 0)
            .map_err(|e| {
                error!("获取视频设备列表失败");
                e
            })?;

        // 若没有视频设备，直接返回空列表
        let Some(enum_moniker) = enum_moniker else {
            return Ok(vec![]);
        };

        let mut i = 0;
        loop {
            let mut moniker = [None];
            let mut fetched = 0;
            let result = enum_moniker.Next(&mut moniker, Some(&mut fetched));
            let moniker = moniker[0].clone();

            if result.is_err() || fetched == 0 || moniker.is_none() {
                break;
            }
            let moniker = moniker.unwrap();

            // 获取属性袋
            let prop_bag: Result<IPropertyBag, windows::core::Error> =
                moniker.BindToStorage(None, None);
            if prop_bag.is_err() {
                continue;
            }
            let prop_bag = prop_bag.unwrap();

            // 从属性中读取摄像头名字
            let name_bstr = BSTR::from("FriendlyName");
            let mut variant = VARIANT::from(BSTR::default());
            let read_result = prop_bag.Read(&name_bstr, &mut variant, None);

            // 获取设备名称
            let camera_name = if read_result.is_err() {
                format!("未知的摄像头 {}", i)
            } else {
                let bstr = variant.Anonymous.Anonymous.Anonymous.bstrVal.clone();
                if bstr.is_empty() {
                    format!("未知的摄像头 {}", i)
                } else {
                    bstr.to_string()
                }
            };

            // 清理VARIANT，释放内部资源
            VariantClear(&mut variant).ok();

            devices.push((camera_name, i));
            i += 1;
        }
    };

    Ok(devices)
}

// 验证摄像头有效性
fn is_camera_index_valid(index: u32) -> opencv::Result<bool> {
    let mut capture = VideoCapture::new(index as i32, opencv::videoio::CAP_ANY)?;
    let is_valid = capture.is_opened()?;

    // 立即释放资源，避免占用摄像头
    if is_valid {
        capture.release()?;
    }

    Ok(is_valid)
}

// 解锁屏幕
pub fn unlock(user_name: String, password: String) -> windows::core::Result<()> {
    {
        // 先连接服务管道
        let client = Client::new(HSTRING::from(r"\\.\pipe\MansonWindowsUnlockRustServer"));
        if client.is_err() {
            return Err(windows::core::Error::new(
                E_UNEXPECTED,
                format!("连接服务管道失败: {:?}", client.err()),
            ));
        }
        let client = client.unwrap();
        if let Err(e) = crate::utils::pipe::write(client.handle, format!("test_data")) {
            return Err(windows::core::Error::new(
                E_UNEXPECTED,
                format!("向服务管道写入数据失败: {:?}", e),
            ));
        }
    }

    // 连接解锁管道，只要2个管道都存在，并且可以写入数据，就认为服务已启动
    let client = Client::new(HSTRING::from(r"\\.\pipe\MansonWindowsUnlockRustUnlock"));
    if client.is_err() {
        return Err(windows::core::Error::new(
            E_UNEXPECTED,
            format!("连接解锁管道失败: {:?}", client.err()),
        ));
    }
    let client = client.unwrap();
    if let Err(e) = crate::utils::pipe::write(
        client.handle,
        format!(
            "unlockFromClient::{}::FaceWinUnlock::{}",
            user_name, password
        ),
    ) {
        return Err(windows::core::Error::new(
            E_UNEXPECTED,
            format!("向解锁管道写入数据失败: {:?}", e),
        ));
    }

    Ok(())
}
