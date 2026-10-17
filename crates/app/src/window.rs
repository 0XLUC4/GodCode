//! Size of the main window, kept between sessions in `window.txt` beside the settings.
//!
//! The memory of eframe is turned off: it saved the size of a window closed while
//! minimized, which is nothing, and the app then opened as a tiny square. Here only a window
//! shown on screen is remembered, and a size too small to use is never brought back.

use std::sync::Mutex;

use egui::Vec2;

/// How the window was last seen on screen.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Remembered {
    /// Size when not maximized, in points; `None` when never seen so.
    pub size: Option<Vec2>,
    pub maximized: bool,
}

static LAST: Mutex<Option<Remembered>> = Mutex::new(None);

/// Note how the window is shown. Call every frame.
pub fn track(ctx: &egui::Context) {
    let (minimized, maximized, inner) = ctx.input(|input| {
        let viewport = input.viewport();
        (
            viewport.minimized.unwrap_or(false),
            viewport.maximized.unwrap_or(false),
            viewport.inner_rect,
        )
    });
    if minimized {
        return;
    }
    let Ok(mut last) = LAST.lock() else {
        return;
    };
    let before = last.unwrap_or_default();
    // A maximized window fills the screen: keep the size it had before.
    let size = if maximized {
        before.size
    } else {
        inner.map(|rect| rect.size()).or(before.size)
    };
    *last = Some(Remembered { size, maximized });
}

/// Write down what [`track`] saw last.
pub fn save() {
    #[cfg(not(target_arch = "wasm32"))]
    {
        let Some(seen) = LAST.lock().ok().and_then(|last| *last) else {
            return;
        };
        let Some(path) = file() else {
            return;
        };
        let size = seen
            .size
            .map_or_else(|| "- -".to_owned(), |size| format!("{} {}", size.x, size.y));
        // Not worth an error message: the window then opens at its usual size.
        let _ = std::fs::write(path, format!("{size} {}", u8::from(seen.maximized)));
    }
}

/// What was written down, when it can be used: a size at least `min` and not absurd.
pub fn load(min: Vec2) -> Remembered {
    #[cfg(not(target_arch = "wasm32"))]
    if let Some(text) = file().and_then(|path| std::fs::read_to_string(path).ok()) {
        return parse(&text, min);
    }
    let _ = min;
    Remembered::default()
}

fn parse(text: &str, min: Vec2) -> Remembered {
    let parts: Vec<&str> = text.split_whitespace().collect();
    let [width, height, maximized] = parts[..] else {
        return Remembered::default();
    };
    let usable = |value: f32, least: f32| value.is_finite() && (least..=20_000.0).contains(&value);
    let size = match (width.parse::<f32>(), height.parse::<f32>()) {
        (Ok(w), Ok(h)) if usable(w, min.x) && usable(h, min.y) => Some(Vec2::new(w, h)),
        _ => None,
    };
    Remembered {
        size,
        maximized: maximized == "1",
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn file() -> Option<std::path::PathBuf> {
    let dir = eframe::storage_dir(crate::APP_NAME)?;
    std::fs::create_dir_all(&dir).ok()?;
    Some(dir.join("window.txt"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tiny_or_broken_size_is_not_brought_back() {
        let min = Vec2::new(760.0, 480.0);
        let ok = parse("1200 800 0", min);
        assert_eq!(ok.size, Some(Vec2::new(1200.0, 800.0)));
        assert!(!ok.maximized);
        assert_eq!(parse("64 64 0", min).size, None);
        let maximized = Remembered {
            size: None,
            maximized: true,
        };
        assert_eq!(parse("- - 1", min), maximized);
        assert_eq!(parse("NaN 900 0", min).size, None);
        assert_eq!(parse("", min), Remembered::default());
    }
}
