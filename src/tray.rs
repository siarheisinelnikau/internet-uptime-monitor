//! A small native Windows message loop; no webview or GUI runtime is bundled.
#[cfg(windows)]
mod windows {
    use crate::{monitor::App, platform};
    use anyhow::{Result, bail};
    use std::{path::PathBuf, ptr, sync::Arc, thread, time::Duration};
    use windows_sys::Win32::{
        Foundation::*,
        System::LibraryLoader::GetModuleHandleW,
        UI::{Shell::*, WindowsAndMessaging::*},
    };

    const CALLBACK: u32 = WM_APP + 1;
    struct Context {
        app: Arc<App>,
        data: PathBuf,
        icon: NOTIFYICONDATAW,
        taskbar_created: u32,
    }
    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(Some(0)).collect()
    }

    unsafe extern "system" fn window_proc(
        window: HWND,
        message: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        // The Box is owned by run(), outlives the window, and is used only on this thread.
        unsafe {
            let context = GetWindowLongPtrW(window, GWLP_USERDATA) as *mut Context;
            if !context.is_null() {
                let context = &mut *context;
                if message == context.taskbar_created {
                    Shell_NotifyIconW(NIM_ADD, &context.icon);
                    return 0;
                }
                if message == CALLBACK {
                    match lparam as u32 {
                        WM_LBUTTONDBLCLK => {
                            context.app.request_window();
                        }
                        WM_RBUTTONUP | WM_CONTEXTMENU => {
                            let menu = CreatePopupMenu();
                            AppendMenuW(menu, MF_STRING, 1001, wide("Open monitor").as_ptr());
                            AppendMenuW(
                                menu,
                                MF_STRING,
                                1002,
                                wide(if context.app.snapshot().paused {
                                    "Resume monitoring"
                                } else {
                                    "Pause monitoring"
                                })
                                .as_ptr(),
                            );
                            if !context.app.snapshot().demo {
                                AppendMenuW(
                                    menu,
                                    MF_STRING
                                        | if platform::startup_enabled() {
                                            MF_CHECKED
                                        } else {
                                            0
                                        },
                                    1003,
                                    wide("Start with Windows").as_ptr(),
                                );
                            }
                            AppendMenuW(menu, MF_SEPARATOR, 0, ptr::null());
                            AppendMenuW(menu, MF_STRING, 1004, wide("Quit monitor").as_ptr());
                            let mut point = POINT::default();
                            GetCursorPos(&mut point);
                            SetForegroundWindow(window);
                            let command = TrackPopupMenu(
                                menu,
                                TPM_RETURNCMD | TPM_RIGHTBUTTON,
                                point.x,
                                point.y,
                                0,
                                window,
                                ptr::null(),
                            );
                            DestroyMenu(menu);
                            PostMessageW(window, WM_NULL, 0, 0);
                            match command {
                                1001 => {
                                    context.app.request_window();
                                }
                                1002 => {
                                    let _ = context.app.control(if context.app.snapshot().paused {
                                        "resume"
                                    } else {
                                        "pause"
                                    });
                                }
                                1003 => {
                                    if let Err(error) = platform::set_startup(
                                        !platform::startup_enabled(),
                                        &context.data,
                                    ) {
                                        *context.app.ui_notice.lock().expect("notice") =
                                            Some(error.to_string());
                                        context.app.request_window();
                                    }
                                }
                                1004 => context.app.stop(),
                                _ => {}
                            }
                        }
                        _ => {}
                    }
                    return 0;
                }
                if message == WM_CLOSE {
                    context.app.stop();
                    return 0;
                }
            }
            DefWindowProcW(window, message, wparam, lparam)
        }
    }

    pub fn run(app: Arc<App>, data: PathBuf) -> Result<()> {
        unsafe {
            let instance = GetModuleHandleW(ptr::null());
            let class_name = wide("InternetUptimeMonitorTray");
            let class = WNDCLASSW {
                lpfnWndProc: Some(window_proc),
                hInstance: instance,
                lpszClassName: class_name.as_ptr(),
                ..Default::default()
            };
            if RegisterClassW(&class) == 0 {
                bail!("Could not register the tray window.");
            }
            let window = CreateWindowExW(
                0,
                class_name.as_ptr(),
                wide("Internet Uptime Monitor").as_ptr(),
                0,
                0,
                0,
                0,
                0,
                ptr::null_mut(),
                ptr::null_mut(),
                instance,
                ptr::null(),
            );
            if window.is_null() {
                bail!("Could not create the tray window.");
            }
            // MAKEINTRESOURCE(1) is an integer-valued resource identifier, never dereferenced.
            #[allow(clippy::manual_dangling_ptr)]
            let resource_id = 1usize as *const u16;
            let mut icon = NOTIFYICONDATAW {
                cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
                hWnd: window,
                uID: 1,
                uFlags: NIF_ICON | NIF_MESSAGE | NIF_TIP,
                uCallbackMessage: CALLBACK,
                hIcon: LoadIconW(instance, resource_id),
                ..Default::default()
            };
            if icon.hIcon.is_null() {
                icon.hIcon = LoadIconW(ptr::null_mut(), IDI_APPLICATION);
            }
            let tip = wide("Internet Uptime Monitor — double-click to open");
            icon.szTip[..tip.len()].copy_from_slice(&tip);
            let taskbar_created = RegisterWindowMessageW(wide("TaskbarCreated").as_ptr());
            let mut context = Box::new(Context {
                app: app.clone(),
                data,
                icon,
                taskbar_created,
            });
            SetWindowLongPtrW(
                window,
                GWLP_USERDATA,
                (&mut *context as *mut Context) as isize,
            );
            if Shell_NotifyIconW(NIM_ADD, &context.icon) == 0 {
                DestroyWindow(window);
                bail!("Could not add the tray icon.");
            }
            app.tray_available
                .store(true, std::sync::atomic::Ordering::Release);
            let mut last_tip = String::new();
            while !app.is_stopping() {
                let mut message = MSG::default();
                while PeekMessageW(&mut message, ptr::null_mut(), 0, 0, PM_REMOVE) != 0 {
                    if message.message == WM_QUIT {
                        app.stop();
                        break;
                    }
                    TranslateMessage(&message);
                    DispatchMessageW(&message);
                }
                let status = app.snapshot();
                let label = if status.demo {
                    "Demo preview"
                } else if status.paused {
                    "Paused"
                } else {
                    status
                        .latest
                        .as_ref()
                        .map(|s| match s.state {
                            crate::model::State::Online => "Connected",
                            crate::model::State::Unknown => "Check needs attention",
                            _ => "Connection interrupted",
                        })
                        .unwrap_or("Checking connection")
                };
                if last_tip != label {
                    last_tip = label.into();
                    context.icon.szTip.fill(0);
                    let tip = wide(&format!("Internet Uptime Monitor — {label}"));
                    context.icon.szTip[..tip.len()].copy_from_slice(&tip);
                    Shell_NotifyIconW(NIM_MODIFY, &context.icon);
                }
                thread::sleep(Duration::from_millis(100));
            }
            Shell_NotifyIconW(NIM_DELETE, &context.icon);
            app.tray_available
                .store(false, std::sync::atomic::Ordering::Release);
            SetWindowLongPtrW(window, GWLP_USERDATA, 0);
            DestroyWindow(window);
        }
        Ok(())
    }
}

#[cfg(windows)]
pub use windows::run;

#[cfg(not(windows))]
pub fn run(
    app: std::sync::Arc<crate::monitor::App>,
    _data: std::path::PathBuf,
) -> anyhow::Result<()> {
    while !app.is_stopping() {
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    Ok(())
}
