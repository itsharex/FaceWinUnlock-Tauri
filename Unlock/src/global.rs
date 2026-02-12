use std::{
    path::PathBuf,
    sync::{atomic::{AtomicBool, AtomicI32, AtomicU32}, Mutex},
};

use log::info;
use r2d2::Pool;
use r2d2_sqlite::SqliteConnectionManager;
use windows::Win32::Foundation::HWND;

pub static EXIT: AtomicBool = AtomicBool::new(false);
pub const LOOP_MILLIS: u64 = 50;
// 是否正在运行面容识别？
pub static IS_RUN: AtomicBool = AtomicBool::new(false);
// 计时器，确定何时调用面容识别代码
pub const TIMER_ID_LOCK_CHECK: usize = 1001;
pub static FACE_RECOG_DELAY: AtomicU32 = AtomicU32::new(10000);

// 是否允许调用面容识别代码？
pub static ALLOW_UNLOCK: AtomicBool = AtomicBool::new(false);

// 全局摄像头索引
pub static CAMERA_INDEX: AtomicI32 = AtomicI32::new(0);
// 面容不匹配时，当前的尝试次数
pub static MATCH_FAIL_COUNT: AtomicI32 = AtomicI32::new(0);

// 最大成功次数，超过这个次数判断为面容匹配
pub const MAX_SUCCESS: usize = 3;
// 最大失败次数，超过这个次数判断为面容不匹配
pub const MAX_FAIL: usize = 3;
// 最大重试次数，这不能让用户自己输入，如果错误次数太多，微软会锁定账户的，很危险
pub const MAX_RETRY: i32 = 3;
// 多长时间进行重试？
pub static RETRY_DELAY: AtomicI32 = AtomicI32::new(10000);
// 是否启用活体检测
pub static LIVENESS_ENABLE: AtomicBool = AtomicBool::new(false);
// 活体检测阈值
pub static LIVENESS_THRESHOLD: AtomicU32 = AtomicU32::new(50);
// 未检测到人脸时多少秒停止面容识别
pub static NOT_FACE_DELAY: AtomicU32 = AtomicU32::new(3);

#[derive(Debug, Clone, Copy)]
pub struct SafeHWND(HWND);
unsafe impl Send for SafeHWND {}
unsafe impl Sync for SafeHWND {}
impl SafeHWND {
    pub fn new(hwnd: HWND) -> Self {
        Self(hwnd)
    }
    pub fn get(&self) -> HWND {
        self.0
    }
    pub fn is_valid(&self) -> bool {
        !self.0.is_invalid()
    }
}

lazy_static::lazy_static! {
    pub static ref DB_POOL: Mutex<Option<Pool<SqliteConnectionManager>>> = Mutex::new(None);
    static ref ROOT_DIR: Mutex<PathBuf> = Mutex::new(PathBuf::new());
    static ref GLOBAL_HWND: Mutex<Option<SafeHWND>> = Mutex::new(None);
    static ref FACE_RECOG_TYPE: Mutex<String> = Mutex::new(String::from("operation"));
    static ref FACE_ALIGNED_TYPE: Mutex<String> = Mutex::new(String::from("default"));
}

// 获取全局路径
pub fn set_global_log_path(path: &str) {
    let path_buf = PathBuf::from(path);
    // 获取父目录，如果没有则使用原路径（兜底）
    let parent_path = path_buf.parent().unwrap_or(&path_buf).to_path_buf();
    let mut global_path = ROOT_DIR.lock().unwrap();
    *global_path = parent_path;
    info!("全局路径设置成功: {}", global_path.display());
}

// 获取全局路径
pub fn get_global_log_path() -> PathBuf {
    let global_path = ROOT_DIR.lock().unwrap();
    global_path.clone() // 返回克隆，避免持有锁过久
}

// 设置全局 HWND
pub fn set_global_hwnd(hwnd: HWND) {
    let mut global_hwnd = GLOBAL_HWND.lock().unwrap();
    *global_hwnd = Some(SafeHWND::new(hwnd));
    info!("全局 HWND 已设置: {:?}", hwnd);
}

// 获取全局 HWND
pub fn get_global_hwnd() -> Option<SafeHWND> {
    let global_hwnd = GLOBAL_HWND.lock().unwrap();
    global_hwnd.clone()
}

// 设置面容识别模式
pub fn set_face_recognition_mode(mode: String) {
    let mut global_face_recognition_mode = FACE_RECOG_TYPE.lock().unwrap();
    *global_face_recognition_mode = mode;
}

// 获取面容识别模式
pub fn get_face_recognition_mode() -> String {
    let global_face_recognition_mode = FACE_RECOG_TYPE.lock().unwrap();
    global_face_recognition_mode.clone()
}

// 设置面容对齐模式
pub fn set_face_aligned_mode(mode: String) {
    let mut global_face_aligned_mode = FACE_ALIGNED_TYPE.lock().unwrap();
    *global_face_aligned_mode = mode;
}

// 获取面容对齐模式
pub fn get_face_aligned_mode() -> String {
    let global_face_aligned_mode = FACE_ALIGNED_TYPE.lock().unwrap();
    global_face_aligned_mode.clone()
}
