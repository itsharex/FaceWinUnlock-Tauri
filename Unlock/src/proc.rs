use std::sync::atomic::Ordering;

use log::{error, info};
use windows::
    Win32::{
        Foundation::{HWND, LPARAM, LRESULT, WPARAM},
        System::
            RemoteDesktop::
                WTSUnRegisterSessionNotification
            
        ,
        UI::WindowsAndMessaging::{
            DefWindowProcW, KillTimer, PostQuitMessage, SetTimer, WM_CREATE, WM_DESTROY, WM_TIMER, WM_WTSSESSION_CHANGE, WTS_SESSION_LOCK, WTS_SESSION_UNLOCK
        },
    }
;

use crate::{face::{prepare_before, run_before}, global::{get_face_recognition_mode, ALLOW_UNLOCK, FACE_RECOG_DELAY, IS_RUN, MATCH_FAIL_COUNT, TIMER_ID_LOCK_CHECK}};

pub fn lock(hwnd: HWND){
    MATCH_FAIL_COUNT.store(0, Ordering::SeqCst);
    match prepare_before() {
        Ok(_) => {
            ALLOW_UNLOCK.store(true, Ordering::SeqCst);
            if get_face_recognition_mode() != "operation" { 
                // 如果是按延迟时间，这里启动定时器
                IS_RUN.store(true, Ordering::SeqCst);
                // 设置一个定时器
                // 当时间到达时，系统会发送 WM_TIMER 消息
                let time_ms = FACE_RECOG_DELAY.load(Ordering::SeqCst);
                unsafe {
                    SetTimer(
                        Some(hwnd),
                        TIMER_ID_LOCK_CHECK,
                        time_ms,
                        None,
                    )
                };

                info!("计时器已设置 {}", time_ms);
            }
        },
        Err(e) => {
            error!("准备工作失败：{}", e);
        }
    }
}

// 窗口过程函数
pub unsafe extern "system" fn window_proc(
    hwnd: HWND,
    msg: u32,
    w_param: WPARAM,
    l_param: LPARAM,
) -> LRESULT {
    match msg {
        // 处理会话状态变更消息
        WM_WTSSESSION_CHANGE => {
            let event_type = w_param.0 as u32;

            match event_type {
                WTS_SESSION_LOCK => {
                    lock(hwnd);
                }
                WTS_SESSION_UNLOCK => {
                    ALLOW_UNLOCK.store(false, Ordering::SeqCst);
                    IS_RUN.store(false, Ordering::SeqCst);
                    // 解锁取消计时器
                    unsafe {
                        let _ = KillTimer(Some(hwnd), TIMER_ID_LOCK_CHECK);
                    };
                }
                _ => {}
            }
            LRESULT(0)
        }

        WM_TIMER => {
            if w_param.0 == TIMER_ID_LOCK_CHECK {
                // 关闭定时器，防止重复触发
                unsafe {
                    let _ = KillTimer(Some(hwnd), TIMER_ID_LOCK_CHECK);
                };
    
                // 二次检查状态
                if IS_RUN.load(Ordering::SeqCst) {
                    run_before();
                }
            }
            LRESULT(0)
        }

        WM_CREATE => LRESULT(0),

        // 处理窗口销毁消息
        WM_DESTROY => {
            // 注销会话通知
            unsafe {
                let _ = WTSUnRegisterSessionNotification(hwnd);
            }
            // 发送退出消息
            unsafe { PostQuitMessage(0) };
            LRESULT(0)
        }

        // 默认消息处理
        _ => unsafe { DefWindowProcW(hwnd, msg, w_param, l_param) },
    }
}