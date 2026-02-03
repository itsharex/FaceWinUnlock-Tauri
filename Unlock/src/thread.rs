use std::{sync::atomic::Ordering, thread::sleep, time::Duration};

use log::{error, info, warn};
use r2d2::Pool;
use r2d2_sqlite::rusqlite;
use windows::{core::HSTRING, Win32::UI::WindowsAndMessaging::{SendMessageW, WM_CLOSE}};

use crate::{face::{run_before, unlock}, global::{get_face_recognition_mode, get_global_hwnd, get_global_log_path, set_global_log_path, DB_POOL, EXIT, IS_RUN, LOOP_MILLIS, MATCH_FAIL_COUNT, MAX_RETRY}, pipe::Server, utils::{can_retry, read_facewinunlock_registry}};

// 管道消息处理
pub fn pipe_message_loop() {
    let mut server = Server::new(HSTRING::from(r"\\.\pipe\MansonWindowsUnlockRustUnlock"));
    loop {
        if EXIT.load(Ordering::SeqCst) {
            break;
        }
        let f_connected = server.connect();
        if f_connected.is_err() {
            error!("管道连接失败：{:?}", f_connected.err());
        } else {
            match crate::pipe::read(server.handle) {
                Ok(content) => {
                    if content == "exit" {
                        info!("收到退出指令");
                        EXIT.store(true, Ordering::SeqCst);
                        if let Some(safe_hwnd) = get_global_hwnd() {
                            let hwnd = safe_hwnd.get();
                            unsafe {
                                SendMessageW(
                                    hwnd,        // 目标 HWND
                                    WM_CLOSE,    // 关闭窗口消息
                                    None,
                                    None
                                );
                            }
                            info!("已向全局 HWND 发送 WM_CLOSE 关闭指令");
                        } else {
                            warn!("全局 HWND 未初始化，无法发送关闭指令");
                        }
                        break;
                    } else if content == "run" {
                        // info!("IS_RUN: {}, MATCH_FAIL_COUNT: {}, {}", IS_RUN.load(Ordering::SeqCst), MATCH_FAIL_COUNT.load(Ordering::SeqCst), get_face_recognition_mode());
                        if !IS_RUN.load(Ordering::SeqCst) && MATCH_FAIL_COUNT.load(Ordering::SeqCst) < MAX_RETRY && get_face_recognition_mode() == "operation"{
                            if can_retry() {
                                IS_RUN.store(true, Ordering::SeqCst);
                                info!("运行面容识别代码");
                                run_before();
                            }
                        }
                    } else if content.contains("unlockFromClient::") {
                        let parts: Vec<&str> = content.split("::").collect();
                        if parts.len() == 4 {
                            let username = parts[1].trim().to_string();
                            let password = parts[3].trim().to_string();

                            if let Err(e) = unlock(username, password) {
                                error!("解锁失败: {:?}", e);
                            }
                        } else {
                            error!("无效的字符串格式，期望 4 段（:: 分隔），实际 {} 段: {}", parts.len(), content);
                        }
                    }
                }
                Err(_e) => {
                    // 先不记了
                    // error!("读取管道数据失败：{:?}", e);
                }
            }
            let _ = server.disconnect();
        }
    }
    info!("管道线程安全卸载完成");
}
// 获取软件安装目录
pub fn get_install_path() {
    loop {
        if EXIT.load(Ordering::SeqCst) {
            break;
        }

        let result = read_facewinunlock_registry("DLL_LOG_PATH");

        if let Ok(log_path_reg) = result.clone() {
            let log_path = if log_path_reg.starts_with("\\\\?\\") {
                log_path_reg["\\\\?\\".len()..].to_string()
            } else {
                log_path_reg
            };

            // 设置全局路径
            set_global_log_path(&log_path);
            break;
        }

        sleep(Duration::from_millis(LOOP_MILLIS));
    }
}
// 连接sqlite
pub fn connect_sqlite() {
    let db_path = get_global_log_path().join("database.db");
    loop {
        if EXIT.load(Ordering::SeqCst) {
            break;
        }
        if db_path.exists() {
            let mut pool_guard = DB_POOL.lock().unwrap();

            if pool_guard.as_ref().is_none() {
                // 如果当前没有SQLite 连接池，则创建一个
                let manager = r2d2_sqlite::SqliteConnectionManager::file(&db_path).with_flags(
                    rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE
                        | rusqlite::OpenFlags::SQLITE_OPEN_FULL_MUTEX,
                );

                let pool = Pool::builder()
                    .max_size(2) // 回调函数使用，不需要太多连接
                    .build(manager)
                    .map_err(|e| {
                        error!("数据库连接池创建失败: {:?}", e);
                        e
                    }).unwrap();

                *pool_guard = Some(pool);
            }

            break;
        }
    }
}
