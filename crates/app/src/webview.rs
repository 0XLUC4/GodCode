//! A web page shown inside the window: the web view of the system (WebView2 on Windows),
//! a window of its own laid over a part of ours. What egui draws there is under it, so it
//! is hidden while a menu or a dialog needs the place.

use egui::Rect;

#[cfg(windows)]
pub struct Web {
    view: wry::WebView,
    url: String,
    /// Where it was last put, in pixels; `None` while it is hidden.
    placed: Option<[i32; 4]>,
}

#[cfg(windows)]
impl Web {
    /// A web view in the window of `frame`, showing `url`. It stays hidden until it is
    /// given a place. `wake` is called when it starts or ends loading a page: there may be
    /// another address to show.
    pub fn open(
        frame: &eframe::Frame,
        url: &str,
        wake: impl Fn() + 'static,
    ) -> Result<Self, String> {
        let view = wry::WebViewBuilder::new()
            .with_url(url)
            .with_on_page_load_handler(move |_, _| wake())
            .with_devtools(true)
            .with_visible(false)
            .build_as_child(frame)
            .map_err(|err| format!("Cannot show the page here: {err}"))?;
        Ok(Self {
            view,
            url: url.to_owned(),
            placed: None,
        })
    }

    /// The address of the page on show: a link followed in it changes it.
    pub fn url(&self) -> String {
        self.view.url().unwrap_or_else(|_| self.url.clone())
    }

    /// Go to the page seen before this one (`back`), or to the one left by going back.
    pub fn step(&self, back: bool) {
        let script = if back {
            "history.back()"
        } else {
            "history.forward()"
        };
        let _ = self.view.evaluate_script(script);
    }

    /// Show another page.
    pub fn go(&mut self, url: &str) {
        if self.view.load_url(url).is_ok() {
            url.clone_into(&mut self.url);
        }
    }

    pub fn reload(&self) {
        // A page that will not reload is still the page: nothing to tell.
        let _ = self.view.reload();
    }

    /// Give the keyboard back to the window: a click in the page took it, and a click
    /// elsewhere does not bring it back by itself.
    pub fn release_keyboard(&self) {
        let _ = self.view.focus_parent();
    }

    /// Open the developer tools of the page: its elements, its console, its requests.
    pub fn inspect(&self) {
        self.view.open_devtools();
    }

    /// Put the view over `rect` (in points), or hide it when there is no place for it.
    pub fn place(&mut self, rect: Option<Rect>, pixels_per_point: f32) {
        let pixels = rect.map(|rect| {
            let scaled = |length: f32| (length * pixels_per_point).round() as i32;
            [
                scaled(rect.min.x),
                scaled(rect.min.y),
                scaled(rect.width()).max(1),
                scaled(rect.height()).max(1),
            ]
        });
        if pixels == self.placed {
            return;
        }
        // The system refusing to move a window is nothing the user can act on.
        if let Some([x, y, width, height]) = pixels {
            let bounds = wry::Rect {
                position: wry::dpi::PhysicalPosition::new(x, y).into(),
                size: wry::dpi::PhysicalSize::new(width as u32, height as u32).into(),
            };
            let _ = self.view.set_bounds(bounds);
        }
        if pixels.is_some() != self.placed.is_some() {
            let _ = self.view.set_visible(pixels.is_some());
        }
        self.placed = pixels;
    }
}

/// Where the system has no web view to lend, pages open in the browser.
#[cfg(not(windows))]
pub struct Web {
    url: String,
}

#[cfg(not(windows))]
impl Web {
    pub fn open(
        _frame: &eframe::Frame,
        _url: &str,
        _wake: impl Fn() + 'static,
    ) -> Result<Self, String> {
        Err("Pages show inside the window on Windows only: opened in the browser".to_owned())
    }

    pub fn url(&self) -> String {
        self.url.clone()
    }

    pub fn step(&self, _back: bool) {}

    pub fn go(&mut self, _url: &str) {}

    pub fn reload(&self) {}

    pub fn inspect(&self) {}

    pub fn release_keyboard(&self) {}

    pub fn place(&mut self, _rect: Option<Rect>, _pixels_per_point: f32) {}
}
