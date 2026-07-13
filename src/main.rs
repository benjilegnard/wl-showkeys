mod devmgr;
mod pango_helper;
mod shm;

use cairo::{Context, Operator, RecordingSurface};
use devmgr::DevMgr;
use input::event::keyboard::{KeyState, KeyboardEventTrait};
use input::{Event, Libinput};
use nix::poll::{poll, PollFd, PollFlags};
use nix::sys::time::{TimeSpec, TimeValLike};
use nix::time::{clock_gettime, ClockId};
use pango_helper::{get_text_size, pango_printf};
use shm::PoolBuffer;
use std::collections::LinkedList;
use std::os::fd::OwnedFd;
use std::os::unix::io::{AsRawFd, RawFd};
// udev Context is not available in this version, we'll handle differently
use wayland_client::protocol::{
    wl_compositor, wl_keyboard, wl_output, wl_registry, wl_seat, wl_shm, wl_surface,
};
use wayland_client::{Connection, Dispatch, QueueHandle};
use wayland_protocols_wlr::layer_shell::v1::client::{zwlr_layer_shell_v1, zwlr_layer_surface_v1};
use xkbcommon::xkb;

const INPUT_DEV_PATH: &str = "/dev/input";

#[derive(Clone)]
struct Keypress {
    name: String,
    utf8: String,
}

struct Output {
    output: wl_output::WlOutput,
    scale: i32,
    subpixel: wl_output::Subpixel,
    name: Option<String>,
}

struct AppState {
    // Device manager
    devmgr: Option<DevMgr>,
    libinput: Option<Libinput>,

    // Appearance
    foreground: u32,
    background: u32,
    specialfg: u32,
    font: String,
    timeout: i64,

    // Wayland
    compositor: Option<wl_compositor::WlCompositor>,
    shm: Option<wl_shm::WlShm>,
    seat: Option<wl_seat::WlSeat>,
    layer_shell: Option<zwlr_layer_shell_v1::ZwlrLayerShellV1>,

    surface: Option<wl_surface::WlSurface>,
    layer_surface: Option<zwlr_layer_surface_v1::ZwlrLayerSurfaceV1>,

    width: u32,
    height: u32,
    frame_scheduled: bool,
    dirty: bool,
    buffers: [PoolBuffer; 2],
    current_output: Option<usize>,
    outputs: Vec<Output>,

    // XKB
    xkb_context: Option<xkb::Context>,
    xkb_keymap: Option<xkb::Keymap>,
    xkb_state: Option<xkb::State>,

    // Keys
    keys: LinkedList<Keypress>,
    last_key: TimeSpec,

    run: bool,
    anchor: u32,
    margin: i32,
    // Horizontal gap (in pixels) inserted between drawn keys.
    padding: u32,

    // Name of the output requested via -o, if any.
    output_name: Option<String>,
}

impl AppState {
    fn new() -> Self {
        Self {
            devmgr: None,
            libinput: None,
            foreground: 0xFFFFFFFF,
            background: 0x000000CC,
            specialfg: 0xAAAAAAFF,
            font: "monospace 24".to_string(),
            timeout: 1,
            compositor: None,
            shm: None,
            seat: None,
            layer_shell: None,
            surface: None,
            layer_surface: None,
            width: 0,
            height: 0,
            frame_scheduled: false,
            dirty: false,
            buffers: [PoolBuffer::new(), PoolBuffer::new()],
            current_output: None,
            outputs: Vec::new(),
            xkb_context: None,
            xkb_keymap: None,
            xkb_state: None,
            keys: LinkedList::new(),
            last_key: TimeSpec::zero(),
            run: true,
            anchor: 0,
            margin: 32,
            padding: 5,
            output_name: None,
        }
    }

    fn set_dirty(&mut self, qh: &QueueHandle<AppState>) {
        if self.frame_scheduled {
            self.dirty = true;
        } else if self.surface.is_some() {
            self.render_frame(qh);
        }
    }

    fn cairo_set_source_u32(cairo: &Context, color: u32) {
        cairo.set_source_rgba(
            ((color >> 24) & 0xFF) as f64 / 255.0,
            ((color >> 16) & 0xFF) as f64 / 255.0,
            ((color >> 8) & 0xFF) as f64 / 255.0,
            (color & 0xFF) as f64 / 255.0,
        );
    }

    fn render_to_cairo(&self, cairo: &Context, scale: i32, width: &mut u32, height: &mut u32) {
        cairo.set_operator(Operator::Source);
        Self::cairo_set_source_u32(cairo, self.background);
        cairo.paint().unwrap();

        for (idx, key) in self.keys.iter().enumerate() {
            // Insert padding between keys (not before the first one).
            if idx > 0 {
                *width += self.padding * scale as u32;
            }

            let mut special = false;
            let name: &str;

            if key.utf8.is_empty() {
                Self::cairo_set_source_u32(cairo, self.specialfg);

                name = match key.name.as_str() {
                    "Return" => "⏎",
                    "Tab" | "ISO_Left_Tab" => "⇥",
                    "BackSpace" => "⇤",
                    "space" => "␣",
                    "Control_L" | "Control_R" => {
                        special = true;
                        "<Ctrl>"
                    }
                    "Alt_L" | "Alt_R" => {
                        special = true;
                        "<Alt>"
                    }
                    "Meta_L" | "Meta_R" => {
                        special = true;
                        "<Alt>"
                    }
                    "ISO_Level3_Shift" => {
                        special = true;
                        "<AltGr>"
                    }
                    "Shift_L" | "Shift_R" => {
                        special = true;
                        "<Shift>"
                    }
                    "Super_L" | "Super_R" => {
                        special = true;
                        "<Super>"
                    }
                    "Caps_Lock" => "⇪",
                    "Escape" => "Esc",
                    "\u{07F0}" => "Del",
                    "Insert" => "Ins",
                    "Prior" => "PgUp",
                    "Next" => "PgDn",
                    "Up" => "↑",
                    "Down" => "↓",
                    "Left" => "←",
                    "Right" => "→",
                    "dead_circumflex" => {
                        special = true;
                        "^"
                    }
                    "dead_tilde" => {
                        special = true;
                        "~"
                    }
                    "dead_grave" => {
                        special = true;
                        "`"
                    }
                    "dead_acute" => {
                        special = true;
                        "´"
                    }
                    "dead_diaeresis" => {
                        special = true;
                        "¨"
                    }
                    _ => &key.name,
                };
            } else {
                Self::cairo_set_source_u32(cairo, self.foreground);
                name = &key.utf8;
            }

            cairo.move_to(*width as f64, 0.0);

            let text = if special {
                format!("{}+", name)
            } else {
                name.to_string()
            };

            let (w, h, _) = get_text_size(cairo, &self.font, scale as f64, &text);
            pango_printf(cairo, &self.font, scale as f64, &text);

            *width += w as u32;
            if *height < h as u32 {
                *height = h as u32;
            }
        }
    }

    fn render_frame(&mut self, qh: &QueueHandle<AppState>) {
        let recorder = RecordingSurface::create(cairo::Content::ColorAlpha, None)
            .expect("Failed to create recording surface");
        let cairo = Context::new(&recorder).expect("Failed to create cairo context");

        cairo.set_antialias(cairo::Antialias::Best);

        // Font options
        let fo = cairo::FontOptions::new().unwrap();
        cairo.set_font_options(&fo);

        cairo.save().unwrap();
        cairo.set_operator(Operator::Clear);
        cairo.paint().unwrap();
        cairo.restore().unwrap();

        let scale = self
            .current_output
            .and_then(|idx| self.outputs.get(idx))
            .map(|o| o.scale)
            .unwrap_or(1);

        let mut width = 0;
        let mut height = 0;
        self.render_to_cairo(&cairo, scale, &mut width, &mut height);

        if height / scale as u32 != self.height
            || width / scale as u32 != self.width
            || self.width == 0
        {
            // Reconfigure surface. Clone the proxy so we can also mutate
            // self.width/self.height without a borrow conflict.
            let surface = self.surface.clone();
            if let Some(surface) = surface {
                if width == 0 || height == 0 {
                    // No content: unmap the surface. wlroots clears the surface's
                    // "configured" state on unmap, so forget our cached size to
                    // force a fresh configure handshake before the next buffer;
                    // otherwise a same-size keypress would commit a buffer onto an
                    // unconfigured surface ("layer_surface has never been configured").
                    surface.attach(None, 0, 0);
                    surface.commit();
                    self.width = 0;
                    self.height = 0;
                } else {
                    if let Some(ref layer_surface) = self.layer_surface {
                        layer_surface.set_size(width / scale as u32, height / scale as u32);
                    }
                    surface.commit();
                }
            }
        } else if height > 0 {
            // Render to shm buffer
            if let Some(ref shm) = self.shm {
                match shm::get_next_buffer(
                    shm,
                    &mut self.buffers,
                    width * scale as u32,
                    height * scale as u32,
                    qh,
                ) {
                    Ok(buffer) => {
                        if let Some(ref shm_cairo) = buffer.cairo {
                            shm_cairo.save().unwrap();
                            shm_cairo.set_operator(Operator::Clear);
                            shm_cairo.paint().unwrap();
                            shm_cairo.restore().unwrap();

                            shm_cairo.set_source_surface(&recorder, 0.0, 0.0).unwrap();
                            shm_cairo.paint().unwrap();

                            if let Some(ref surface) = self.surface {
                                surface.set_buffer_scale(scale);
                                if let Some(ref buf) = buffer.buffer {
                                    surface.attach(Some(buf), 0, 0);
                                }
                                surface.damage_buffer(0, 0, self.width as i32, self.height as i32);
                                surface.commit();
                            }
                        }
                    }
                    Err(e) => {
                        eprintln!("Failed to get buffer: {}", e);
                    }
                }
            }
        }
    }

    fn handle_libinput_event(&mut self, event: Event) {
        if self.xkb_state.is_none() {
            return;
        }

        if let Event::Keyboard(kb_event) = event {
            let keycode = xkb::Keycode::new(kb_event.key() + 8);
            let key_state = kb_event.key_state();

            if let Some(ref mut xkb_state) = self.xkb_state {
                xkb_state.update_key(
                    keycode,
                    match key_state {
                        KeyState::Released => xkb::KeyDirection::Up,
                        KeyState::Pressed => xkb::KeyDirection::Down,
                    },
                );

                if key_state == KeyState::Pressed {
                    let keysym = xkb_state.key_get_one_sym(keycode);
                    let name = xkb::keysym_get_name(keysym);
                    let utf8 = xkb_state.key_get_utf8(keycode);

                    let utf8_filtered = if utf8.chars().all(|c| c > ' ') {
                        utf8
                    } else {
                        String::new()
                    };

                    let keypress = Keypress {
                        name,
                        utf8: utf8_filtered,
                    };

                    self.keys.push_back(keypress);
                }
            }

            self.last_key = clock_gettime(ClockId::CLOCK_MONOTONIC).unwrap();
            // Note: set_dirty requires QueueHandle which we don't have here
            // Will need to be called from the event loop
        }
    }
}

impl Dispatch<wl_registry::WlRegistry, ()> for AppState {
    fn event(
        state: &mut Self,
        registry: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let wl_registry::Event::Global {
            name,
            interface,
            version,
        } = event
        {
            match &interface[..] {
                "wl_compositor" => {
                    state.compositor = Some(registry.bind::<wl_compositor::WlCompositor, _, _>(
                        name,
                        version.min(4),
                        qh,
                        (),
                    ));
                }
                "wl_shm" => {
                    state.shm =
                        Some(registry.bind::<wl_shm::WlShm, _, _>(name, version.min(1), qh, ()));
                }
                "wl_seat" => {
                    let seat = registry.bind::<wl_seat::WlSeat, _, _>(name, version.min(5), qh, ());
                    state.seat = Some(seat);
                }
                "zwlr_layer_shell_v1" => {
                    state.layer_shell = Some(
                        registry.bind::<zwlr_layer_shell_v1::ZwlrLayerShellV1, _, _>(
                            name,
                            version.min(1),
                            qh,
                            (),
                        ),
                    );
                }
                "wl_output" => {
                    // Bind v4 when available so we receive the `name` event
                    // (connector name, e.g. "DP-1") used by the -o option.
                    let output =
                        registry.bind::<wl_output::WlOutput, _, _>(name, version.min(4), qh, ());
                    state.outputs.push(Output {
                        output,
                        scale: 1,
                        subpixel: wl_output::Subpixel::Unknown,
                        name: None,
                    });
                }
                _ => {}
            }
        }
    }
}

// Implement required Dispatch traits for Wayland protocols
impl Dispatch<wayland_client::protocol::wl_shm_pool::WlShmPool, ()> for AppState {
    fn event(
        _: &mut Self,
        _: &wayland_client::protocol::wl_shm_pool::WlShmPool,
        _: wayland_client::protocol::wl_shm_pool::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<wayland_client::protocol::wl_buffer::WlBuffer, ()> for AppState {
    fn event(
        state: &mut Self,
        buffer: &wayland_client::protocol::wl_buffer::WlBuffer,
        event: wayland_client::protocol::wl_buffer::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wayland_client::protocol::wl_buffer::Event::Release = event {
            // Mark buffer as not busy
            for buf in &mut state.buffers {
                if let Some(ref b) = buf.buffer {
                    if b == buffer {
                        buf.busy = false;
                        break;
                    }
                }
            }
        }
    }
}

impl Dispatch<wl_compositor::WlCompositor, ()> for AppState {
    fn event(
        _: &mut Self,
        _: &wl_compositor::WlCompositor,
        _: wl_compositor::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<wl_shm::WlShm, ()> for AppState {
    fn event(
        _: &mut Self,
        _: &wl_shm::WlShm,
        _: wl_shm::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<wl_seat::WlSeat, ()> for AppState {
    fn event(
        _state: &mut Self,
        seat: &wl_seat::WlSeat,
        event: wl_seat::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let wl_seat::Event::Capabilities { capabilities } = event {
            if let wayland_client::WEnum::Value(caps) = capabilities {
                if caps.contains(wl_seat::Capability::Keyboard) {
                    seat.get_keyboard(qh, ());
                }
            }
        }
    }
}

impl Dispatch<wl_keyboard::WlKeyboard, ()> for AppState {
    fn event(
        state: &mut Self,
        _: &wl_keyboard::WlKeyboard,
        event: wl_keyboard::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_keyboard::Event::Keymap { format, fd, size } = event {
            if let wayland_client::WEnum::Value(fmt) = format {
                if fmt == wl_keyboard::KeymapFormat::XkbV1 {
                    use nix::sys::mman::{mmap, munmap, MapFlags, ProtFlags};
                    use std::num::NonZeroUsize;

                    let len = match NonZeroUsize::new(size as usize) {
                        Some(l) => l,
                        None => return,
                    };

                    // mmap the keymap read-only. `fd` is borrowed here and stays
                    // owned by the event, so it is closed exactly once on drop.
                    let map = unsafe {
                        mmap(
                            None,
                            len,
                            ProtFlags::PROT_READ,
                            MapFlags::MAP_PRIVATE,
                            &fd,
                            0,
                        )
                    };

                    if let Ok(ptr) = map {
                        let bytes = unsafe {
                            std::slice::from_raw_parts(ptr.as_ptr() as *const u8, size as usize)
                        };
                        // Wayland NUL-terminates the keymap within `size`; xkbcommon
                        // must not see the trailing NUL or any padding after it.
                        let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
                        if let Ok(text) = std::str::from_utf8(&bytes[..end]) {
                            if let Some(ref context) = state.xkb_context {
                                if let Some(keymap) = xkb::Keymap::new_from_string(
                                    context,
                                    text.to_string(),
                                    xkb::KEYMAP_FORMAT_TEXT_V1,
                                    xkb::KEYMAP_COMPILE_NO_FLAGS,
                                ) {
                                    state.xkb_state = Some(xkb::State::new(&keymap));
                                    state.xkb_keymap = Some(keymap);
                                }
                            }
                        }
                        unsafe {
                            let _ = munmap(ptr, size as usize);
                        }
                    }
                }
            }
        }
    }
}

impl Dispatch<wl_output::WlOutput, ()> for AppState {
    fn event(
        state: &mut Self,
        output: &wl_output::WlOutput,
        event: wl_output::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let Some(idx) = state.outputs.iter().position(|o| o.output == *output) {
            match event {
                wl_output::Event::Geometry { subpixel, .. } => {
                    if let wayland_client::WEnum::Value(sp) = subpixel {
                        state.outputs[idx].subpixel = sp;
                    }
                }
                wl_output::Event::Scale { factor } => {
                    state.outputs[idx].scale = factor;
                }
                wl_output::Event::Name { name } => {
                    state.outputs[idx].name = Some(name);
                }
                _ => {}
            }
        }
    }
}

impl Dispatch<wl_surface::WlSurface, ()> for AppState {
    fn event(
        state: &mut Self,
        _: &wl_surface::WlSurface,
        event: wl_surface::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_surface::Event::Enter { output } = event {
            if let Some(idx) = state.outputs.iter().position(|o| o.output == output) {
                state.current_output = Some(idx);
            }
        }
    }
}

impl Dispatch<zwlr_layer_shell_v1::ZwlrLayerShellV1, ()> for AppState {
    fn event(
        _: &mut Self,
        _: &zwlr_layer_shell_v1::ZwlrLayerShellV1,
        _: zwlr_layer_shell_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<zwlr_layer_surface_v1::ZwlrLayerSurfaceV1, ()> for AppState {
    fn event(
        state: &mut Self,
        layer_surface: &zwlr_layer_surface_v1::ZwlrLayerSurfaceV1,
        event: zwlr_layer_surface_v1::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        match event {
            zwlr_layer_surface_v1::Event::Configure {
                serial,
                width,
                height,
            } => {
                state.width = width;
                state.height = height;
                layer_surface.ack_configure(serial);
                // Ack, then render synchronously (like the C original) so the
                // ack always reaches the compositor before any buffer commit.
                state.set_dirty(qh);
            }
            zwlr_layer_surface_v1::Event::Closed => {
                state.run = false;
            }
            _ => {}
        }
    }
}

fn parse_color(color: &str) -> u32 {
    let color = color.strip_prefix('#').unwrap_or(color);

    let len = color.len();
    if len != 6 && len != 8 {
        eprintln!("Invalid color {}, defaulting to 0xFFFFFFFF", color);
        return 0xFFFFFFFF;
    }

    let mut res = u32::from_str_radix(color, 16).unwrap_or(0xFFFFFFFF);
    if len == 6 {
        res = (res << 8) | 0xFF;
    }
    res
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Start device manager (runs as root initially)
    let devmgr = DevMgr::start(INPUT_DEV_PATH)?;
    let devmgr_fd = devmgr.fd;

    // Now running as normal user
    let mut state = AppState::new();

    // Parse command-line arguments
    let args: Vec<String> = std::env::args().collect();
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "-b" => {
                i += 1;
                if i < args.len() {
                    state.background = parse_color(&args[i]);
                }
            }
            "-f" => {
                i += 1;
                if i < args.len() {
                    state.foreground = parse_color(&args[i]);
                }
            }
            "-s" => {
                i += 1;
                if i < args.len() {
                    state.specialfg = parse_color(&args[i]);
                }
            }
            "-F" => {
                i += 1;
                if i < args.len() {
                    state.font = args[i].clone();
                }
            }
            "-t" => {
                i += 1;
                if i < args.len() {
                    state.timeout = args[i].parse().unwrap_or(1);
                }
            }
            "-a" => {
                i += 1;
                if i < args.len() {
                    use wayland_protocols_wlr::layer_shell::v1::client::zwlr_layer_surface_v1::Anchor;
                    state.anchor |= match args[i].as_str() {
                        "top" => Anchor::Top.bits(),
                        "left" => Anchor::Left.bits(),
                        "right" => Anchor::Right.bits(),
                        "bottom" => Anchor::Bottom.bits(),
                        _ => 0,
                    };
                }
            }
            "-m" => {
                i += 1;
                if i < args.len() {
                    state.margin = args[i].parse().unwrap_or(32);
                }
            }
            "-p" => {
                i += 1;
                if i < args.len() {
                    state.padding = args[i].parse().unwrap_or(5);
                }
            }
            "-o" => {
                i += 1;
                if i < args.len() {
                    state.output_name = Some(args[i].clone());
                }
            }
            "-h" | "--help" => {
                eprintln!(
                    "usage: wl-showkeys [-b|-f|-s #RRGGBB[AA]] [-F font] [-t timeout]\n\t[-a top|left|right|bottom] [-m margin] [-p padding] [-o output]"
                );
                return Ok(());
            }
            _ => {}
        }
        i += 1;
    }

    // Initialize udev and libinput. Devices are opened through the privileged
    // devmgr child, since we've dropped root and can't open /dev/input directly.
    struct Interface {
        devmgr_fd: RawFd,
    }
    impl input::LibinputInterface for Interface {
        fn open_restricted(&mut self, path: &std::path::Path, _flags: i32) -> Result<OwnedFd, i32> {
            let path_str = path.to_str().ok_or(libc::EINVAL)?;
            match devmgr::open_device(self.devmgr_fd, path_str) {
                Ok(fd) => Ok(unsafe { OwnedFd::from_raw_fd(fd) }),
                Err(_) => Err(libc::EACCES),
            }
        }
        fn close_restricted(&mut self, fd: OwnedFd) {
            drop(fd); // OwnedFd automatically closes on drop
        }
    }
    let mut libinput = Libinput::new_with_udev(Interface { devmgr_fd });
    libinput
        .udev_assign_seat("seat0")
        .map_err(|_| "Failed to assign seat")?;
    state.libinput = Some(libinput);

    // Initialize XKB
    state.xkb_context = Some(xkb::Context::new(xkb::CONTEXT_NO_FLAGS));

    // Connect to Wayland
    let conn = Connection::connect_to_env()?;
    let display = conn.display();
    let mut event_queue = conn.new_event_queue();
    let qh = event_queue.handle();

    let _registry = display.get_registry(&qh, ());
    event_queue
        .roundtrip(&mut state)
        .map_err(|e| format!("Roundtrip error: {:?}", e))?;

    // Check for required globals
    if state.compositor.is_none() {
        return Err("wl_compositor not available".into());
    }
    if state.shm.is_none() {
        return Err("wl_shm not available".into());
    }
    if state.layer_shell.is_none() {
        return Err("zwlr_layer_shell_v1 not available".into());
    }

    event_queue
        .roundtrip(&mut state)
        .map_err(|e| format!("Roundtrip error: {:?}", e))?;

    // Create surface and layer surface
    if let (Some(ref compositor), Some(ref layer_shell)) = (&state.compositor, &state.layer_shell) {
        let surface = compositor.create_surface(&qh, ());

        // Resolve the output requested via -o (by connector name, e.g. "DP-1").
        // If unset we pass None and let the compositor pick; if the requested
        // name is not found we warn and fall back to the default.
        let output = match &state.output_name {
            Some(wanted) => {
                let found = state
                    .outputs
                    .iter()
                    .find(|o| o.name.as_deref() == Some(wanted.as_str()));
                if found.is_none() {
                    eprintln!(
                        "Warning: output '{}' not found, using compositor default",
                        wanted
                    );
                }
                found.map(|o| &o.output)
            }
            None => None,
        };

        let layer_surface = layer_shell.get_layer_surface(
            &surface,
            output,
            zwlr_layer_shell_v1::Layer::Top,
            "showkeys".to_string(),
            &qh,
            (),
        );

        use wayland_protocols_wlr::layer_shell::v1::client::zwlr_layer_surface_v1::Anchor;
        layer_surface.set_size(1, 1);
        layer_surface.set_anchor(Anchor::from_bits_truncate(state.anchor));
        layer_surface.set_margin(state.margin, state.margin, state.margin, state.margin);
        layer_surface.set_exclusive_zone(-1);

        surface.commit();

        state.surface = Some(surface);
        state.layer_surface = Some(layer_surface);
    }

    // Store devmgr
    state.devmgr = Some(devmgr);

    // Receive and acknowledge the initial configure before entering the loop,
    // so the surface is guaranteed to be configured before we attach a buffer.
    event_queue
        .roundtrip(&mut state)
        .map_err(|e| format!("Roundtrip error: {:?}", e))?;

    // Main event loop
    let wl_fd_raw = event_queue
        .prepare_read()
        .unwrap()
        .connection_fd()
        .as_raw_fd();
    let libinput_fd_raw = state.libinput.as_ref().unwrap().as_raw_fd();

    // Create borrowed FDs for poll
    use std::os::fd::{BorrowedFd, FromRawFd};

    while state.run {
        // Flush Wayland events
        let _ = event_queue.flush();

        let timeout = if state.keys.is_empty() {
            None
        } else {
            Some(100u16)
        };

        let libinput_fd = unsafe { BorrowedFd::borrow_raw(libinput_fd_raw) };
        let wl_fd = unsafe { BorrowedFd::borrow_raw(wl_fd_raw) };

        let mut pollfds = [
            PollFd::new(libinput_fd, PollFlags::POLLIN),
            PollFd::new(wl_fd, PollFlags::POLLIN),
        ];

        if poll(&mut pollfds, timeout).is_ok() {
            // Clear old keys
            let now = clock_gettime(ClockId::CLOCK_MONOTONIC).unwrap();
            if !state.keys.is_empty() && now.tv_sec() >= state.last_key.tv_sec() + state.timeout {
                state.keys.clear();
                let qh = event_queue.handle();
                state.set_dirty(&qh);
            }

            // Handle libinput events
            if pollfds[0]
                .revents()
                .unwrap_or(PollFlags::empty())
                .contains(PollFlags::POLLIN)
            {
                // Collect events first to avoid borrow checker issues
                let mut events = Vec::new();
                if let Some(ref mut libinput) = state.libinput {
                    libinput.dispatch().unwrap();
                    while let Some(event) = libinput.next() {
                        events.push(event);
                    }
                }

                // Process events
                let qh = event_queue.handle();
                for event in events {
                    state.handle_libinput_event(event);
                }
                if !state.keys.is_empty() {
                    state.set_dirty(&qh);
                }
            }

            // Handle Wayland events
            if pollfds[1]
                .revents()
                .unwrap_or(PollFlags::empty())
                .contains(PollFlags::POLLIN)
            {
                event_queue.blocking_dispatch(&mut state)?;
            }
        }
    }

    // Cleanup
    if let Some(devmgr) = state.devmgr.take() {
        devmgr.finish();
    }

    Ok(())
}
