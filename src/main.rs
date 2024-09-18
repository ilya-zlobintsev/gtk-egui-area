mod egui_area;

use egui_area::EguiArea;
use gtk::{prelude::*, Application, ApplicationWindow};
use std::{cell::RefCell, ptr};

fn main() {
    {
        #[cfg(target_os = "macos")]
        let library = unsafe { libloading::os::unix::Library::new("libepoxy.0.dylib") }.unwrap();
        #[cfg(all(unix, not(target_os = "macos")))]
        let library = unsafe { libloading::os::unix::Library::new("libepoxy.so.0") }.unwrap();
        #[cfg(windows)]
        let library = libloading::os::windows::Library::open_already_loaded("libepoxy-0.dll")
            .or_else(|_| libloading::os::windows::Library::open_already_loaded("epoxy-0.dll"))
            .unwrap();

        epoxy::load_with(|name| {
            unsafe { library.get::<_>(name.as_bytes()) }
                .map(|symbol| *symbol)
                .unwrap_or(ptr::null())
        });
    }

    let app = Application::builder().build();

    app.connect_activate(build_ui);

    app.run();
}

fn build_ui(app: &Application) {
    let window = ApplicationWindow::new(app);
    window.set_default_height(600);
    window.set_default_width(600);

    let demo_windows = RefCell::new(egui_demo_lib::DemoWindows::default());

    let egui_area = EguiArea::new(move |ctx| {
        // egui::CentralPanel::default().show(ctx, |ui| {
        // ui.label(format!("Hello world! Area size: {:?}", ctx.screen_rect()));
        // if ui.button("Click me").clicked() {
        //     // take some action here
        // }
        demo_windows.borrow_mut().ui(ctx);
        // });
    });
    // egui_area.egui_ctx().style_mut(|style| {
    //     style.visuals = egui::Visuals::light();
    // });
    /*egui_area.egui_ctx().set_zoom_factor(1.5);*/

    let frame = gtk::Frame::new(Some("EGUI"));
    frame.set_label_align(0.5);
    frame.set_margin_top(10);
    frame.set_margin_bottom(10);
    frame.set_margin_start(10);
    frame.set_margin_end(10);
    frame.set_child(Some(&egui_area));

    window.set_child(Some(&frame));

    window.present();
}
