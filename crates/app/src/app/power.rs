//! Developer toolbox, regex lab, coding time and the memory GodCode uses: the small tools
//! people otherwise look for on websites or in extensions.

use godcode_core::activity::{self, Activity};
use godcode_core::regex_lab::{self, Flags};
use godcode_core::toolbox;

use super::*;

/// Without a key, a click or a scroll for this long, the time stops counting.
const IDLE_AFTER: f64 = 120.0;
/// Seconds of coding gathered before they are added to the record.
const FLUSH_EVERY: f64 = 5.0;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tool {
    Jwt,
    Base64,
    Url,
    Json,
    Time,
    Uuid,
    Hash,
}

impl Tool {
    const ALL: [Tool; 7] = [
        Tool::Jwt,
        Tool::Base64,
        Tool::Url,
        Tool::Json,
        Tool::Time,
        Tool::Uuid,
        Tool::Hash,
    ];

    fn title(self) -> &'static str {
        match self {
            Tool::Jwt => "JWT",
            Tool::Base64 => "Base64",
            Tool::Url => "URL",
            Tool::Json => "JSON",
            Tool::Time => "Unix time",
            Tool::Uuid => "UUID",
            Tool::Hash => "Hash",
        }
    }
}

struct Toolbox {
    tool: Tool,
    input: String,
    /// Encode (true) or decode; for JSON: pretty (true) or minified.
    forward: bool,
    uuids: Vec<String>,
}

struct RegexLab {
    pattern: String,
    flags: Flags,
    text: String,
    replacement: String,
}

pub(super) struct Power {
    toolbox: Option<Toolbox>,
    regex: Option<RegexLab>,
    time_open: bool,
    pub(super) activity: Activity,
    /// Until when what is done counts as coding (egui time).
    active_until: f64,
    last_frame: f64,
    gathered: f64,
    /// Bytes of memory in use, and when they were read.
    pub(super) memory: Option<u64>,
    memory_read: f64,
}

impl Power {
    pub(super) fn load(saved: Option<String>) -> Self {
        Self {
            toolbox: None,
            regex: None,
            time_open: false,
            activity: saved.map(|text| Activity::parse(&text)).unwrap_or_default(),
            active_until: 0.0,
            last_frame: 0.0,
            gathered: 0.0,
            memory: None,
            memory_read: f64::NEG_INFINITY,
        }
    }
}

/// Today in the time of this computer, `YYYY-MM-DD`.
fn local_date() -> String {
    #[cfg(windows)]
    {
        #[repr(C)]
        #[derive(Default)]
        struct SystemTime {
            year: u16,
            month: u16,
            day_of_week: u16,
            day: u16,
            hour: u16,
            minute: u16,
            second: u16,
            milliseconds: u16,
        }
        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn GetLocalTime(time: *mut SystemTime);
        }
        let mut now = SystemTime::default();
        // SAFETY: GetLocalTime only writes the struct it is given, which lives here.
        unsafe { GetLocalTime(&mut now) };
        format!("{:04}-{:02}-{:02}", now.year, now.month, now.day)
    }
    #[cfg(not(windows))]
    {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |time| time.as_secs() as i64);
        toolbox::timestamp_to_date(now)[..10].to_owned()
    }
}

/// Memory this program uses (its working set), when the system tells.
fn memory_in_use() -> Option<u64> {
    #[cfg(windows)]
    {
        #[repr(C)]
        #[derive(Default)]
        struct Counters {
            cb: u32,
            page_fault_count: u32,
            peak_working_set: usize,
            working_set: usize,
            quota_peak_paged_pool: usize,
            quota_paged_pool: usize,
            quota_peak_non_paged_pool: usize,
            quota_non_paged_pool: usize,
            pagefile: usize,
            peak_pagefile: usize,
        }
        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn GetCurrentProcess() -> *mut std::ffi::c_void;
            fn K32GetProcessMemoryInfo(
                process: *mut std::ffi::c_void,
                counters: *mut Counters,
                size: u32,
            ) -> i32;
        }
        let mut counters = Counters {
            cb: std::mem::size_of::<Counters>() as u32,
            ..Counters::default()
        };
        // SAFETY: the pseudo handle of this process needs no closing; the struct is ours and
        // its size is given.
        let done =
            unsafe { K32GetProcessMemoryInfo(GetCurrentProcess(), &mut counters, counters.cb) };
        (done != 0).then_some(counters.working_set as u64)
    }
    #[cfg(not(windows))]
    {
        None
    }
}

fn looks_like_base64(text: &str) -> bool {
    let text = text.trim();
    text.len() >= 4
        && !text.contains(' ')
        && toolbox::base64_decode(text).is_ok_and(|bytes| String::from_utf8(bytes).is_ok())
}

/// The `count` days that end today, oldest first.
fn last_days(count: usize) -> Vec<String> {
    let today = toolbox::date_to_timestamp(&local_date()).unwrap_or(0);
    (0..count)
        .rev()
        .map(|back| toolbox::timestamp_to_date(today - back as i64 * 86_400)[..10].to_owned())
        .collect()
}

fn output_box(ui: &mut Ui, text: &str, error: bool) {
    let palette = Palette::of(ui);
    ui.horizontal(|ui| {
        ui.label(RichText::new("Result").color(palette.text_dim));
        if !error
            && !text.is_empty()
            && widgets::small_icon_button(ui, icon::COPY, "Copy").clicked()
        {
            ui.ctx().copy_text(text.to_owned());
        }
    });
    let color = if error { palette.error } else { palette.text };
    let mut shown = text.to_owned();
    ScrollArea::vertical()
        .id_salt("toolbox-out")
        .max_height(260.0)
        .show(ui, |ui| {
            ui.add(
                egui::TextEdit::multiline(&mut shown)
                    .code_editor()
                    .text_color(color)
                    .desired_width(f32::INFINITY)
                    .desired_rows(6)
                    .interactive(true),
            );
        });
}

impl GodApp {
    fn selected_text(&self) -> String {
        self.tabs
            .get(self.active)
            .filter(|tab| !tab.selection.is_empty())
            .map(|tab| {
                tab.text
                    .chars()
                    .skip(tab.selection.start)
                    .take(tab.selection.len())
                    .collect()
            })
            .unwrap_or_default()
    }

    // ------------------------------------------------------------ coding time and memory

    /// Count the time spent while keys, clicks or scrolls come in; read the memory now and then.
    pub(super) fn track_activity(&mut self, ctx: &Context) {
        let (now, acting, focused) = ctx.input(|input| {
            let acting = input.events.iter().any(|event| {
                matches!(
                    event,
                    egui::Event::Key { .. }
                        | egui::Event::Text(_)
                        | egui::Event::Paste(_)
                        | egui::Event::PointerButton { .. }
                        | egui::Event::MouseWheel { .. }
                )
            });
            (input.time, acting, input.viewport().focused.unwrap_or(true))
        });
        if acting && focused {
            self.power.active_until = now + IDLE_AFTER;
        }
        let step = (now - self.power.last_frame).clamp(0.0, 5.0);
        self.power.last_frame = now;
        let coding = focused && now < self.power.active_until && !self.tabs.is_empty();
        if coding {
            self.power.gathered += step;
        }
        if self.power.gathered >= FLUSH_EVERY || (!coding && self.power.gathered >= 1.0) {
            let project = self
                .folder()
                .and_then(|root| {
                    root.file_name()
                        .map(|name| name.to_string_lossy().into_owned())
                })
                .unwrap_or_else(|| "No folder".to_owned());
            let language = self
                .tabs
                .get(self.active)
                .map_or("Other", |tab| tab.language.name);
            let seconds = self.power.gathered as u64;
            self.power.gathered -= seconds as f64;
            self.power
                .activity
                .add(&local_date(), &project, language, seconds);
        }
        if now - self.power.memory_read > 2.0 {
            self.power.memory_read = now;
            self.power.memory = memory_in_use();
        }
    }

    pub(super) fn open_time(&mut self) {
        self.power.time_open = true;
    }

    pub(super) fn time_window(&mut self, ctx: &Context) {
        if !self.power.time_open {
            return;
        }
        let palette = Palette::of_ctx(ctx);
        let mut open = true;
        let days = last_days(14);
        let week = &days[days.len() - 7..];
        let month = last_days(30);
        let activity = &self.power.activity;
        egui::Window::new(with_icon(icon::TIMER, "Coding time"))
            .id(Id::new("time-window"))
            .open(&mut open)
            .anchor(Align2::CENTER_CENTER, vec2(0.0, 0.0))
            .default_size(vec2(640.0, 520.0))
            .show(ctx, |ui| {
                let dim = |text: &str| RichText::new(text.to_owned()).color(palette.text_dim);
                let today = activity.day_total(days.last().map_or("", String::as_str));
                let this_week: u64 = activity.totals(week).iter().map(|(_, seconds)| seconds).sum();
                ui.horizontal(|ui| {
                    ui.label(RichText::new(activity::duration(today)).size(26.0).strong());
                    ui.label(dim("today"));
                    ui.add_space(24.0);
                    ui.label(RichText::new(activity::duration(this_week)).size(26.0).strong());
                    ui.label(dim("these 7 days"));
                });
                ui.label(dim("Counted while you type, click or scroll in GodCode; kept on this computer only."));
                ui.add_space(10.0);
                // Fourteen bars, one per day.
                let totals = activity.totals(&days);
                let most = totals.iter().map(|(_, seconds)| *seconds).max().unwrap_or(0).max(1);
                let height = 110.0;
                let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), height + 18.0), Sense::hover());
                let slot = rect.width() / totals.len() as f32;
                for (index, (date, seconds)) in totals.iter().enumerate() {
                    let left = rect.left() + slot * index as f32;
                    let bar = height * (*seconds as f32 / most as f32);
                    let shape = egui::Rect::from_min_max(
                        egui::pos2(left + slot * 0.2, rect.top() + height - bar),
                        egui::pos2(left + slot * 0.8, rect.top() + height),
                    );
                    let fill = if index + 1 == totals.len() { palette.accent } else { palette.accent.gamma_multiply(0.55) };
                    ui.painter().rect_filled(shape, 3.0, fill);
                    ui.painter().text(
                        egui::pos2(left + slot / 2.0, rect.top() + height + 10.0),
                        Align2::CENTER_CENTER,
                        &date[8..],
                        egui::FontId::proportional(11.0),
                        palette.text_dim,
                    );
                    let hover = egui::Rect::from_min_max(egui::pos2(left, rect.top()), egui::pos2(left + slot, rect.bottom()));
                    ui.interact(hover, Id::new(("time-day", index)), Sense::hover())
                        .on_hover_text(format!("{date}: {}", activity::duration(*seconds)));
                }
                ui.add_space(8.0);
                ui.columns(2, |columns| {
                    let lists = [
                        ("PROJECTS, 30 DAYS", activity.by_project(&month)),
                        ("LANGUAGES, 30 DAYS", activity.by_language(&month)),
                    ];
                    for (column, (title, list)) in columns.iter_mut().zip(lists) {
                        column.label(dim(title));
                        if list.is_empty() {
                            column.label("Nothing yet");
                        }
                        for (name, seconds) in list.iter().take(8) {
                            column.horizontal(|ui| {
                                ui.label(name);
                                ui.label(dim(&activity::duration(*seconds)));
                            });
                        }
                    }
                });
            });
        self.power.time_open = open;
    }

    // ------------------------------------------------------------ toolbox

    pub(super) fn open_toolbox(&mut self) {
        let selected = self.selected_text();
        // A selection that looks like a token or a date opens on the tool that reads it.
        let tool = match selected.trim() {
            text if text.matches('.').count() == 2 && text.starts_with("eyJ") => Tool::Jwt,
            text if text.starts_with('{') || text.starts_with('[') => Tool::Json,
            text if !text.is_empty() && text.chars().all(|ch| ch.is_ascii_digit()) => Tool::Time,
            text if text.contains('%') => Tool::Url,
            _ => Tool::Base64,
        };
        // Text that already is encoded is decoded first.
        let encoded = match tool {
            Tool::Url => true,
            Tool::Base64 => looks_like_base64(&selected),
            _ => false,
        };
        let forward = !encoded;
        self.power.toolbox = Some(Toolbox {
            tool,
            input: selected,
            forward,
            uuids: Vec::new(),
        });
    }

    pub(super) fn toolbox_window(&mut self, ctx: &Context) {
        let Some(mut shown) = self.power.toolbox.take() else {
            return;
        };
        let mut open = true;
        egui::Window::new(with_icon(icon::TOOLBOX, "Developer toolbox"))
            .id(Id::new("toolbox-window"))
            .open(&mut open)
            .anchor(Align2::CENTER_CENTER, vec2(0.0, 0.0))
            .default_size(vec2(720.0, 560.0))
            .show(ctx, |ui| {
                ui.horizontal_wrapped(|ui| {
                    for tool in Tool::ALL {
                        if ui
                            .selectable_label(shown.tool == tool, tool.title())
                            .clicked()
                        {
                            shown.tool = tool;
                        }
                    }
                });
                ui.add_space(6.0);
                let switch = match shown.tool {
                    Tool::Base64 | Tool::Url => Some(("Encode", "Decode")),
                    Tool::Json => Some(("Pretty", "Minify")),
                    _ => None,
                };
                if let Some((on, off)) = switch {
                    ui.horizontal(|ui| {
                        ui.radio_value(&mut shown.forward, true, on);
                        ui.radio_value(&mut shown.forward, false, off);
                    });
                }
                if shown.tool == Tool::Uuid {
                    ui.horizontal(|ui| {
                        if ui.button(with_icon(icon::PLUS, "New UUID")).clicked() {
                            shown.uuids.insert(0, toolbox::uuid_v4());
                        }
                        if ui.button("Ten more").clicked() {
                            for _ in 0..10 {
                                shown.uuids.insert(0, toolbox::uuid_v4());
                            }
                        }
                    });
                    if shown.uuids.is_empty() {
                        shown.uuids.push(toolbox::uuid_v4());
                    }
                    output_box(ui, &shown.uuids.join("\n"), false);
                    return;
                }
                if shown.tool == Tool::Time && ui.button(with_icon(icon::CLOCK, "Now")).clicked() {
                    let now = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map_or(0, |time| time.as_secs());
                    shown.input = now.to_string();
                }
                let hint = match shown.tool {
                    Tool::Jwt => "Paste a token: eyJhbGciOi…",
                    Tool::Time => "A Unix time (1760450000) or a date (2026-10-14 16:30)",
                    Tool::Hash => "Text to hash",
                    _ => "Text",
                };
                ScrollArea::vertical()
                    .id_salt("toolbox-in")
                    .max_height(180.0)
                    .show(ui, |ui| {
                        ui.add(
                            egui::TextEdit::multiline(&mut shown.input)
                                .code_editor()
                                .hint_text(hint)
                                .desired_width(f32::INFINITY)
                                .desired_rows(5),
                        );
                    });
                ui.add_space(6.0);
                let input = shown.input.as_str();
                if input.trim().is_empty() {
                    return;
                }
                let result: Result<String, String> = match shown.tool {
                    Tool::Base64 if shown.forward => Ok(toolbox::base64_encode(input.as_bytes())),
                    Tool::Base64 => toolbox::base64_decode(input)
                        .map(|bytes| String::from_utf8_lossy(&bytes).into_owned()),
                    Tool::Url if shown.forward => Ok(toolbox::url_encode(input)),
                    Tool::Url => toolbox::url_decode(input),
                    Tool::Json if shown.forward => toolbox::json_pretty(input),
                    Tool::Json => toolbox::json_minify(input),
                    Tool::Time => {
                        let text = input.trim();
                        match text.parse::<i64>() {
                            Ok(stamp) => Ok(toolbox::timestamp_to_date(stamp)),
                            Err(_) => {
                                toolbox::date_to_timestamp(text).map(|stamp| stamp.to_string())
                            }
                        }
                    }
                    Tool::Hash => Ok(toolbox::hashes(input)
                        .into_iter()
                        .map(|(name, hash)| format!("{name:<8} {hash}"))
                        .collect::<Vec<_>>()
                        .join("\n")),
                    Tool::Jwt => toolbox::jwt_decode(input).map(|jwt| {
                        let now = std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .map_or(0, |time| time.as_secs() as i64);
                        let mut text = String::new();
                        if let Some(expires) = jwt.expires {
                            let state = if expires < now {
                                "EXPIRED"
                            } else {
                                "valid until"
                            };
                            text.push_str(&format!(
                                "{state} {}\n",
                                toolbox::timestamp_to_date(expires)
                            ));
                        }
                        if let Some(issued) = jwt.issued {
                            text.push_str(&format!(
                                "issued {}\n",
                                toolbox::timestamp_to_date(issued)
                            ));
                        }
                        format!(
                            "{text}\nHEADER\n{}\n\nPAYLOAD\n{}\n\n(the signature is not checked)",
                            jwt.header, jwt.payload
                        )
                    }),
                    Tool::Uuid => Ok(String::new()),
                };
                match result {
                    Ok(text) => output_box(ui, &text, false),
                    Err(err) => output_box(ui, &err, true),
                }
            });
        if open {
            self.power.toolbox = Some(shown);
        }
    }

    // ------------------------------------------------------------ regex lab

    pub(super) fn open_regex(&mut self) {
        let selected = self.selected_text();
        let text = if selected.is_empty() {
            "Ada Lovelace <ada@math.org>, born 1815-12-10\nAlan Turing <alan@bletchley.uk>, born 1912-06-23".to_owned()
        } else {
            selected
        };
        self.power.regex = Some(RegexLab {
            pattern: r"(?P<user>\w+)@(?P<host>[\w.]+)".to_owned(),
            flags: Flags::default(),
            text,
            replacement: String::new(),
        });
    }

    pub(super) fn regex_window(&mut self, ctx: &Context) {
        let Some(mut shown) = self.power.regex.take() else {
            return;
        };
        let palette = Palette::of_ctx(ctx);
        let mut open = true;
        egui::Window::new(with_icon(icon::ASTERISK, "Regex lab"))
            .id(Id::new("regex-window"))
            .open(&mut open)
            .anchor(Align2::CENTER_CENTER, vec2(0.0, 0.0))
            .default_size(vec2(760.0, 600.0))
            .show(ctx, |ui| {
                let dim = |text: &str| RichText::new(text.to_owned()).color(palette.text_dim);
                ui.add(
                    egui::TextEdit::singleline(&mut shown.pattern)
                        .code_editor()
                        .hint_text("Regular expression")
                        .desired_width(f32::INFINITY)
                        .margin(widgets::FIELD_MARGIN),
                );
                ui.horizontal(|ui| {
                    ui.checkbox(&mut shown.flags.ignore_case, "Ignore case");
                    ui.checkbox(&mut shown.flags.multi_line, "^ $ per line");
                    ui.checkbox(&mut shown.flags.dot_all, ". matches line breaks");
                });
                let found = regex_lab::run(&shown.pattern, shown.flags, &shown.text);
                match &found {
                    Ok(found) => {
                        let count = if found.len() >= regex_lab::MAX_FOUND {
                            format!("{}+ matches", regex_lab::MAX_FOUND)
                        } else if found.len() == 1 {
                            "1 match".to_owned()
                        } else {
                            format!("{} matches", found.len())
                        };
                        ui.label(RichText::new(count).strong());
                    }
                    Err(err) => {
                        ui.colored_label(palette.error, err.lines().last().unwrap_or(err));
                    }
                }
                ui.label(dim("TEXT"));
                // The text with its matches marked, as it is typed.
                let marks: Vec<std::ops::Range<usize>> = found
                    .as_ref()
                    .map(|found| found.iter().map(|found| found.range.clone()).collect())
                    .unwrap_or_default();
                let mark_fill = palette.accent.gamma_multiply(0.35);
                let text_color = palette.text;
                let mut layouter = move |ui: &egui::Ui, text: &dyn egui::TextBuffer, wrap: f32| {
                    let text = text.as_str();
                    let font = egui::FontId::monospace(13.0);
                    let mut job = LayoutJob::default();
                    job.wrap.max_width = wrap;
                    let mut at = 0;
                    for mark in &marks {
                        if mark.start < at || mark.end > text.len() {
                            continue;
                        }
                        job.append(
                            &text[at..mark.start],
                            0.0,
                            egui::TextFormat::simple(font.clone(), text_color),
                        );
                        let mut format = egui::TextFormat::simple(font.clone(), text_color);
                        format.background = mark_fill;
                        job.append(&text[mark.start..mark.end], 0.0, format);
                        at = mark.end;
                    }
                    job.append(&text[at..], 0.0, egui::TextFormat::simple(font, text_color));
                    ui.fonts_mut(|fonts| fonts.layout_job(job))
                };
                ScrollArea::vertical()
                    .id_salt("regex-text")
                    .max_height(180.0)
                    .show(ui, |ui| {
                        ui.add(
                            egui::TextEdit::multiline(&mut shown.text)
                                .desired_width(f32::INFINITY)
                                .desired_rows(6)
                                .layouter(&mut layouter),
                        );
                    });
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    ui.label(dim("Replace with"));
                    ui.add(
                        egui::TextEdit::singleline(&mut shown.replacement)
                            .code_editor()
                            .hint_text("$1, ${user}…")
                            .desired_width(f32::INFINITY)
                            .margin(widgets::FIELD_MARGIN),
                    );
                });
                if !shown.replacement.is_empty()
                    && let Ok(replaced) = regex_lab::replace(
                        &shown.pattern,
                        shown.flags,
                        &shown.text,
                        &shown.replacement,
                    )
                {
                    ui.label(RichText::new(replaced).monospace());
                }
                if let Ok(found) = &found
                    && !found.is_empty()
                {
                    ui.add_space(6.0);
                    ui.label(dim("MATCHES"));
                    ScrollArea::vertical()
                        .id_salt("regex-found")
                        .max_height(200.0)
                        .show(ui, |ui| {
                            egui::Grid::new("regex-matches")
                                .striped(true)
                                .show(ui, |ui| {
                                    for (index, found) in found.iter().enumerate().take(200) {
                                        ui.label(dim(&format!("{}", index + 1)));
                                        ui.label(RichText::new(&found.text).monospace().strong());
                                        let groups: Vec<String> = found
                                            .groups
                                            .iter()
                                            .map(|group| format!("{} = {}", group.name, group.text))
                                            .collect();
                                        ui.label(RichText::new(groups.join("   ")).monospace());
                                        ui.end_row();
                                    }
                                });
                        });
                }
            });
        if open {
            self.power.regex = Some(shown);
        }
    }
}
