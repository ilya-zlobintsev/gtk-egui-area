use gtk::{prelude::*, glib, Application, ApplicationWindow};
use gtk_egui_area::EguiArea;
use std::{cell::RefCell, rc::Rc};

fn main() {
    unsafe {
        std::env::set_var("GDK_BACKEND", "wayland,x11");
    }

    let app = Application::builder().build();

    app.connect_activate(build_ui);

    app.run();
}

#[derive(Default)]
struct SharedState {
    text: String,
    checked: bool,
}

fn build_ui(app: &Application) {
    let window = ApplicationWindow::new(app);
    window.set_default_width(400);
    window.set_default_height(300);

    let root_container = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    root_container.set_homogeneous(true);

    let shared_state = Rc::new(RefCell::new(SharedState::default()));

    let gtk_controls = gtk::Box::new(gtk::Orientation::Vertical, 5);

    let gtk_text_entry = gtk::Entry::new();
    gtk_text_entry.connect_changed({
        let shared_state = shared_state.clone();
        move |entry| {
            if let Ok(mut state) = shared_state.try_borrow_mut() {
                state.text = entry.text().to_string();
            }
        }
    });

    let gtk_check_button = gtk::CheckButton::new();
    gtk_check_button.set_label(Some("GTK CheckButton"));
    gtk_check_button.connect_toggled({
        let shared_state = shared_state.clone();
        move |button| {
            if let Ok(mut state) = shared_state.try_borrow_mut() {
                state.checked = button.is_active();
            }
        }
    });

    gtk_controls.append(&gtk_text_entry);
    gtk_controls.append(&gtk_check_button);

    root_container.append(&gtk_controls);

    let egui_area = EguiArea::new(move |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            let mut state = shared_state.borrow_mut();

            let text_edit_response = ui.text_edit_singleline(&mut state.text);
            if text_edit_response.changed() {
                gtk_text_entry.set_text(&state.text);
            }

            let checkbox_response = ui.checkbox(&mut state.checked, "EGUI Checkbox");
            if checkbox_response.changed() {
                gtk_check_button.set_active(state.checked);
            }
        });
    });
    egui_area.egui_ctx().set_zoom_factor(1.2);
    root_container.append(&egui_area);

    window.set_child(Some(&root_container));

    // recommended
    let app_clone = app.clone();
    window.connect_close_request(move |w| {
        w.hide(); // stop gtk to send draw events, prevent egui to use destroyed gl context
        let app_clone = app_clone.clone();
        glib::MainContext::default().spawn_local(async move {
            app_clone.quit();
        });
        glib::Propagation::Stop
    });

    window.present();
}
