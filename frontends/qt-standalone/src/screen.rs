//! The running game: `GameScreen` owns the machine, advances it on the QML timer's ticks and
//! pushes each new frame to its `FrameItem` base (`cpp/frame.h`).

#[cxx_qt::bridge]
pub mod qobject {
    unsafe extern "C++" {
        include!("cxx-qt-lib/qstring.h");
        type QString = cxx_qt_lib::QString;
        include!("cxx-qt-lib/qimage.h");
        type QImage = cxx_qt_lib::QImage;
    }

    unsafe extern "C++Qt" {
        include!("frame.h");
        #[qobject]
        type FrameItem;
    }

    unsafe extern "RustQt" {
        #[qobject]
        #[qml_element]
        #[base = FrameItem]
        #[qproperty(QString, status)]
        #[qproperty(bool, running)]
        type GameScreen = super::GameScreenRust;

        #[inherit]
        #[cxx_name = "showImage"]
        fn show_image(self: Pin<&mut Self>, image: &QImage);

        #[inherit]
        fn clear(self: Pin<&mut Self>);

        /// Opens the first package at `path`; indices as in `Inspector` (0 is automatic).
        #[qinvokable]
        fn open(self: Pin<&mut Self>, path: &QString, version: i32, resolution: i32);

        /// One step of the frontend loop: advance the clock by the real time elapsed and show
        /// the frame if the game pushed a new one.
        #[qinvokable]
        fn tick(self: Pin<&mut Self>);

        /// A Qt key (`Qt::Key`) went down or up. Returns whether it maps to a handset key.
        #[qinvokable]
        #[cxx_name = "keyEvent"]
        fn key_event(self: Pin<&mut Self>, key: i32, pressed: bool) -> bool;
    }

    // Without this the generated constructor passes the `QObject*` parent to the base, which
    // wants a `QQuickItem*`.
    impl cxx_qt::Initialize for GameScreen {}
}

use std::pin::Pin;
use std::time::Instant;

use cxx_qt::CxxQtType;
use cxx_qt_lib::{QImage, QImageFormat, QString};

use brewx::machine::{Machine, avk};

/// The longest step one tick may take: after a stall (window dragged, debugger) the game
/// resumes instead of fast-forwarding through the lost time.
const MAX_STEP_MS: u64 = 100;

#[derive(Default)]
pub struct GameScreenRust {
    status: QString,
    running: bool,
    machine: Option<Machine>,
    last_tick: Option<Instant>,
    shown_updates: u64,
}

impl cxx_qt::Initialize for qobject::GameScreen {
    fn initialize(self: Pin<&mut Self>) {}
}

impl qobject::GameScreen {
    fn open(mut self: Pin<&mut Self>, path: &QString, version: i32, resolution: i32) {
        let config = crate::bridge::launch_config(version, resolution);
        let path = std::path::PathBuf::from(path.to_string());
        let started = brewx::package::discover(&path)
            .map_err(|e| e.to_string())
            .and_then(|pkgs| {
                pkgs.into_iter()
                    .next()
                    .ok_or("nenhum .mod encontrado".to_owned())
            })
            .and_then(|pkg| brewx::start(&pkg, &config).map(|(r, m)| (pkg, r, m)));
        let status = match started {
            Ok((pkg, resolved, machine)) => {
                let status = format!(
                    "{} — BREW {} — {}",
                    pkg.name, resolved.version.value, resolved.resolution.value
                );
                let mut rust = self.as_mut().rust_mut();
                rust.machine = Some(machine);
                rust.last_tick = None;
                rust.shown_updates = u64::MAX;
                status
            }
            Err(e) => {
                self.as_mut().rust_mut().machine = None;
                self.as_mut().clear();
                format!("erro: {e}")
            }
        };
        self.as_mut().set_status(QString::from(&status));
        let running = self.machine.is_some();
        self.as_mut().set_running(running);
        self.as_mut().tick();
    }

    fn tick(mut self: Pin<&mut Self>) {
        let now = Instant::now();
        let elapsed = self
            .last_tick
            .map_or(0, |t| now.duration_since(t).as_millis() as u64)
            .min(MAX_STEP_MS);
        let mut rust = self.as_mut().rust_mut();
        rust.last_tick = Some(now);
        let shown = rust.shown_updates;
        let Some(machine) = rust.machine.as_mut() else {
            return;
        };
        machine.advance(elapsed);
        let updates = machine.updates();
        let halted = machine.halted().map(str::to_owned);
        let frame = (updates != shown).then(|| {
            let screen = machine.screen();
            let (w, h) = (screen.width() as i32, screen.height() as i32);
            // RGBA8888 rows are always 4-byte aligned, as QImage requires.
            unsafe { QImage::from_raw_bytes(screen.to_rgba(), w, h, QImageFormat::Format_RGBA8888) }
        });
        rust.shown_updates = updates;
        if let Some(image) = frame {
            self.as_mut().show_image(&image);
        }
        if let Some(reason) = halted
            && self.running
        {
            let status = format!("{} — parou: {reason}", self.status);
            self.as_mut().set_status(QString::from(&status));
            self.as_mut().set_running(false);
        }
    }

    fn key_event(mut self: Pin<&mut Self>, key: i32, pressed: bool) -> bool {
        let Some(code) = handset_key(key) else {
            return false;
        };
        if let Some(machine) = self.as_mut().rust_mut().machine.as_mut() {
            machine.key(code, pressed);
        }
        true
    }
}

/// Keyboard → handset keypad: arrows, Enter/Space for select, Backspace/Esc for clear, F1/F2
/// for the soft keys, and the digits, `*` and `#` as themselves.
fn handset_key(qt_key: i32) -> Option<u32> {
    const KEY_ESCAPE: i32 = 0x0100_0000;
    const KEY_BACKSPACE: i32 = 0x0100_0003;
    const KEY_RETURN: i32 = 0x0100_0004;
    const KEY_ENTER: i32 = 0x0100_0005;
    const KEY_LEFT: i32 = 0x0100_0012;
    const KEY_UP: i32 = 0x0100_0013;
    const KEY_RIGHT: i32 = 0x0100_0014;
    const KEY_DOWN: i32 = 0x0100_0015;
    const KEY_F1: i32 = 0x0100_0030;
    const KEY_F2: i32 = 0x0100_0031;
    Some(match qt_key {
        KEY_UP => avk::UP,
        KEY_DOWN => avk::DOWN,
        KEY_LEFT => avk::LEFT,
        KEY_RIGHT => avk::RIGHT,
        KEY_RETURN | KEY_ENTER | 0x20 => avk::SELECT,
        KEY_BACKSPACE | KEY_ESCAPE => avk::CLR,
        KEY_F1 => avk::SOFT1,
        KEY_F2 => avk::SOFT2,
        0x2a => avk::STAR,
        0x23 => avk::POUND,
        0x30..=0x39 => avk::ZERO + (qt_key - 0x30) as u32,
        _ => return None,
    })
}
