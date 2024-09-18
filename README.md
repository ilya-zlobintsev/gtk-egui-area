# gtk-egui-area

This library provides an `EguiArea` widget for integrating egui inside of GTK applications. It uses the gtk `GLArea` widget as the base with the [egui_glow](https://github.com/emilk/egui/tree/master/crates/egui_glow) renderer to draw inside of it.

![image](https://github.com/user-attachments/assets/76ad5af6-848c-4400-a2a1-8247c1ed36b4)

See [demo](./examples/demo.rs) for usage example.

Supported features:
- Input handling (Keyboard/Mouse/Touchpad were tested)
- Clipboard support
- HiDPI Display handling
- Opening URLs

Not supported:
- Accessibility

# Requirements

- `gtk-rs`
- `egui` (also re-exported from this library)
- `libepoxy` - epoxy is a dependency of GTK, so you should already have it, but this library loads it explicitly so it should be available in the standard library paths
