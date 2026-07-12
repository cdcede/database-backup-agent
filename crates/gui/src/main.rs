#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod views;
mod ipc_client;


use app::BackupAgentApp;
use eframe::egui;

fn main() -> eframe::Result {
    // Initialize standard logging
    tracing_subscriber::fmt::init();

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([950.0, 620.0])
            .with_min_inner_size([750.0, 500.0])
            .with_title("Backup Agent"),
        ..Default::default()
    };


    let result = eframe::run_native(
        "Backup Agent",
        options,
        Box::new(|cc| Ok(Box::new(BackupAgentApp::new(cc)))),
    );

    if let Err(e) = &result {
        tracing::error!("eframe::run_native failed: {e}");
        // windows_subsystem = "windows" means there is no console to see the
        // error above when launched by double-click, so surface it visibly.
        // The most common cause is the graphics driver only exposing OpenGL
        // below the 2.0 floor egui_glow requires (typical on physical
        // servers with basic/BMC video chips) — point at the fix.
        #[cfg(windows)]
        show_startup_error_dialog(&e.to_string());
    }

    result
}

#[cfg(windows)]
fn show_startup_error_dialog(details: &str) {
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_ICONERROR, MB_OK};

    let message = format!(
        "Backup Agent no pudo iniciar la interfaz gráfica.\n\n\
         Detalle: {details}\n\n\
         Si el detalle menciona OpenGL, es probable que la tarjeta/controlador de video \
         de este equipo no soporte OpenGL 2.0+ (común en servidores físicos). Solución:\n\
         1. Descarga \"opengl32.dll\" de un paquete Mesa3D para Windows \
         (https://fdossena.com/?p=mesa%2Findex.frag o github.com/pal1000/mesa-dist-win).\n\
         2. Cópialo en la misma carpeta que backup-agent-gui.exe.\n\
         3. Vuelve a abrir el programa.\n\n\
         Ver installer/README.md, sección Troubleshooting, para más detalle."
    );

    let to_wide = |s: &str| -> Vec<u16> {
        OsStr::new(s).encode_wide().chain(std::iter::once(0)).collect()
    };
    let title_w = to_wide("Backup Agent — Error de inicio");
    let message_w = to_wide(&message);

    unsafe {
        MessageBoxW(
            0,
            message_w.as_ptr(),
            title_w.as_ptr(),
            MB_OK | MB_ICONERROR,
        );
    }
}
