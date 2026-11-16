//! Bash ISE — a small PowerShell-ISE-style environment for bash scripts.
//!
//! How this file is organised:
//!   1. Theme        – colours and widget styling
//!   2. Highlighter  – bash tokenizer -> coloured text for the editor
//!   3. Console      – the output buffer shown in the blue pane
//!   4. Runner       – starts bash on background threads, streams its output
//!   5. Tabs         – one open document per tab
//!   6. App          – state, actions (what the user asked for) and the UI

use eframe::egui;
use egui::text::{CCursor, CCursorRange, LayoutJob, TextFormat};
use egui::{Color32, FontId, Key, KeyboardShortcut, Margin, Modifiers, RichText};
use std::borrow::Cow;
use std::io::{Read, Write};
use std::ops::Range;
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::path::{Path, PathBuf};
use std::process::{ChildStdin, Command, Stdio};
use std::sync::mpsc::{sync_channel, Receiver, SyncSender, TryRecvError};
use std::thread;
use std::time::{Duration, Instant};

fn main() -> eframe::Result {
    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Bash ISE")
            .with_inner_size([1100.0, 760.0])
            .with_min_inner_size([640.0, 420.0]),
        ..Default::default()
    };
    eframe::run_native(
        "Bash ISE",
        native_options,
        Box::new(|cc| Ok(Box::new(BashIse::new(cc)))),
    )
}

// ─────────────────────────────────────────────────────────────────────────────
// 1. Theme
// ─────────────────────────────────────────────────────────────────────────────

const EDITOR_FONT_SIZE: f32 = 14.0;
const CONSOLE_FONT_SIZE: f32 = 13.5;

// The console keeps the classic ISE navy in both light and dark mode.
const CONSOLE_BG: Color32 = Color32::from_rgb(1, 36, 86);
const CONSOLE_BAR: Color32 = Color32::from_rgb(1, 27, 66);
const CONSOLE_FG: Color32 = Color32::from_rgb(236, 238, 244);
const CONSOLE_ERR: Color32 = Color32::from_rgb(255, 128, 118);
const CONSOLE_DIM: Color32 = Color32::from_rgb(142, 168, 210);
const CONSOLE_PROMPT: Color32 = Color32::from_rgb(255, 214, 102);
const CONSOLE_OK: Color32 = Color32::from_rgb(134, 214, 150);

/// Every colour the app picks itself (egui's own widgets are styled in `setup_style`).
#[derive(Clone, Copy)]
struct Palette {
    chrome: Color32,       // menu bar, toolbar, status bar
    tab_bar: Color32,      // strip behind the tabs
    editor_bg: Color32,
    gutter_fg: Color32,    // line numbers
    current_line: Color32, // band behind the line the cursor is on
    find_hit: Color32,
    accent: Color32,
    run: Color32,
    stop: Color32,
    // syntax
    text: Color32,
    comment: Color32,
    string: Color32,
    variable: Color32,
    keyword: Color32,
    command: Color32,
    number: Color32,
    operator: Color32,
    param: Color32,
}

impl Palette {
    fn of(dark: bool) -> Self {
        let c = Color32::from_rgb;
        if dark {
            Self {
                chrome: c(37, 40, 48),
                tab_bar: c(28, 30, 37),
                editor_bg: c(22, 24, 29),
                gutter_fg: c(94, 102, 117),
                current_line: c(32, 35, 43),
                find_hit: c(110, 86, 20),
                accent: c(76, 157, 243),
                run: c(110, 200, 120),
                stop: c(240, 113, 103),
                text: c(214, 217, 224),
                comment: c(106, 153, 85),
                string: c(206, 145, 120),
                variable: c(255, 158, 100),
                keyword: c(86, 156, 214),
                command: c(229, 192, 123),
                number: c(181, 206, 168),
                operator: c(150, 160, 175),
                param: c(156, 220, 254),
            }
        } else {
            // Light mode borrows the PowerShell ISE colours.
            Self {
                chrome: c(240, 241, 244),
                tab_bar: c(226, 228, 233),
                editor_bg: c(255, 255, 255),
                gutter_fg: c(150, 156, 166),
                current_line: c(243, 246, 251),
                find_hit: c(255, 226, 130),
                accent: c(0, 103, 192),
                run: c(22, 140, 50),
                stop: c(200, 50, 40),
                text: c(30, 33, 40),
                comment: c(0, 128, 0),
                string: c(139, 0, 0),
                variable: c(232, 72, 0),
                keyword: c(0, 0, 139),
                command: c(0, 0, 255),
                number: c(128, 0, 128),
                operator: c(105, 105, 105),
                param: c(0, 0, 128),
            }
        }
    }

    fn token(&self, tok: Tok) -> Color32 {
        match tok {
            Tok::Plain => self.text,
            Tok::Comment => self.comment,
            Tok::Str => self.string,
            Tok::Var => self.variable,
            Tok::Keyword => self.keyword,
            Tok::Command => self.command,
            Tok::Number => self.number,
            Tok::Operator => self.operator,
            Tok::Param => self.param,
        }
    }
}

/// Style egui's own widgets (buttons, menus, panels…) for both themes once at startup.
fn setup_style(ctx: &egui::Context) {
    for (theme, dark) in [(egui::Theme::Dark, true), (egui::Theme::Light, false)] {
        let p = Palette::of(dark);
        ctx.style_mut_of(theme, |style| {
            style.spacing.item_spacing = egui::vec2(8.0, 6.0);
            style.spacing.button_padding = egui::vec2(9.0, 4.0);
            style.spacing.menu_margin = Margin::same(6);

            let v = &mut style.visuals;
            v.panel_fill = p.chrome;
            v.window_fill = p.chrome;
            v.extreme_bg_color = p.editor_bg;
            v.selection.bg_fill = p.accent.gamma_multiply(if dark { 0.45 } else { 0.30 });
            v.selection.stroke = egui::Stroke::new(1.0, p.accent);
            v.hyperlink_color = p.accent;
            v.window_corner_radius = 8.into();
            v.menu_corner_radius = 8.into();
            for w in [
                &mut v.widgets.noninteractive,
                &mut v.widgets.inactive,
                &mut v.widgets.hovered,
                &mut v.widgets.active,
                &mut v.widgets.open,
            ] {
                w.corner_radius = 5.into();
            }
        });
    }
}

/// Inside the console pane we always want light-on-navy widgets.
fn apply_console_visuals(ui: &mut egui::Ui) {
    let mut v = egui::Visuals::dark();
    v.override_text_color = Some(CONSOLE_FG);
    v.extreme_bg_color = CONSOLE_BG;
    v.selection.bg_fill = Color32::from_rgb(40, 96, 180);
    v.widgets.inactive.weak_bg_fill = Color32::from_rgb(20, 58, 116);
    v.widgets.hovered.weak_bg_fill = Color32::from_rgb(34, 78, 146);
    v.widgets.active.weak_bg_fill = Color32::from_rgb(44, 92, 166);
    *ui.visuals_mut() = v;
}

// ─────────────────────────────────────────────────────────────────────────────
// 2. Highlighter
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Debug)]
enum Tok {
    Plain,
    Comment,
    Str,
    Var,
    Keyword,
    Command,
    Number,
    Operator,
    Param,
}

/// Split a bash script into coloured pieces. The pieces always cover the whole
/// input, in order, so they can be glued back together into the editor text.
///
/// This is a highlighter, not a parser: it understands quotes, `$variables`,
/// `$(command substitution)`, heredocs, `case` patterns and "is this word a
/// command or an argument?", which is enough to colour real-world scripts well.
fn tokenize(src: &str) -> Vec<(Range<usize>, Tok)> {
    let mut lexer = Lexer {
        src,
        b: src.as_bytes(),
        pos: 0,
        out: Vec::new(),
        heredocs: Vec::new(),
    };
    while lexer.pos < lexer.b.len() {
        lexer.run(false);
    }
    lexer.out
}

struct Lexer<'a> {
    src: &'a str,
    b: &'a [u8],
    pos: usize,
    out: Vec<(Range<usize>, Tok)>,
    /// Heredocs announced on the current line: (delimiter, strip leading tabs).
    heredocs: Vec<(String, bool)>,
}

/// Characters that end a word.
fn is_meta(c: u8) -> bool {
    matches!(
        c,
        b' ' | b'\t' | b'\n' | b'\r' | b'|' | b'&' | b';' | b'(' | b')' | b'<' | b'>' | b'\''
            | b'"' | b'$' | b'`' | b'\\'
    )
}

/// Length of a `$variable` starting at `b[0] == b'$'`, if there is one.
fn var_len(b: &[u8]) -> Option<usize> {
    match *b.get(1)? {
        b'{' => {
            let end = b.iter().position(|&c| c == b'}' || c == b'\n')?;
            (b[end] == b'}').then_some(end + 1)
        }
        c if c.is_ascii_alphabetic() || c == b'_' => {
            let name = b[1..]
                .iter()
                .take_while(|c| c.is_ascii_alphanumeric() || **c == b'_');
            Some(1 + name.count())
        }
        c if c.is_ascii_digit() || b"@*#?$!-".contains(&c) => Some(2),
        _ => None,
    }
}

/// For `NAME=…`, `NAME+=…` or `NAME[i]=…` return (length of NAME, index of '=').
fn assignment(word: &str) -> Option<(usize, usize)> {
    let b = word.as_bytes();
    if !(b.first()?.is_ascii_alphabetic() || b[0] == b'_') {
        return None;
    }
    let name = b
        .iter()
        .take_while(|c| c.is_ascii_alphanumeric() || **c == b'_')
        .count();
    let mut i = name;
    if b.get(i) == Some(&b'[') {
        i += b[i..].iter().position(|&c| c == b']')? + 1;
    }
    if b.get(i) == Some(&b'+') {
        i += 1;
    }
    (b.get(i) == Some(&b'=')).then_some((name, i))
}

impl Lexer<'_> {
    /// Emit everything from `start` up to the current position as `tok`.
    fn emit(&mut self, start: usize, tok: Tok) {
        if start == self.pos {
            return;
        }
        // Same colour as the piece before? Then just grow that piece.
        let previous = self.out.last_mut().filter(|last| last.1 == tok && last.0.end == start);
        match previous {
            Some(last) => last.0.end = self.pos,
            None => self.out.push((start..self.pos, tok)),
        }
    }

    fn peek(&self, offset: usize) -> u8 {
        *self.b.get(self.pos + offset).unwrap_or(&0)
    }

    /// Advance over one whole character (which may be several UTF-8 bytes).
    fn bump_char(&mut self) {
        self.pos += 1;
        while self.pos < self.b.len() && (self.b[self.pos] & 0xC0) == 0x80 {
            self.pos += 1;
        }
    }

    /// Tokenize commands. With `until_paren` we are inside `$( … )` and return
    /// right after the closing parenthesis.
    fn run(&mut self, until_paren: bool) {
        let mut cmd_pos = true; // is the next word a command name?
        let mut depth = 0usize; // open parentheses
        let mut case_depth = 0usize; // nested case … esac
        let mut pattern_pos = false; // inside a `pattern)` of a case
        let mut expect_case_in = false; // saw `case`, waiting for its `in`
        let mut decl = false; // after local/export/…: words are variable names
        let mut loop_var = false; // after for/select: next word is a variable

        while self.pos < self.b.len() {
            let start = self.pos;
            let c = self.b[start];
            match c {
                b'\n' => {
                    self.pos += 1;
                    self.emit(start, Tok::Plain);
                    if !self.heredocs.is_empty() {
                        self.heredoc_bodies();
                    }
                    cmd_pos = true;
                    decl = false;
                }
                b' ' | b'\t' | b'\r' => {
                    self.pos += 1;
                    self.emit(start, Tok::Plain);
                }
                b'#' => {
                    while self.pos < self.b.len() && self.b[self.pos] != b'\n' {
                        self.pos += 1;
                    }
                    self.emit(start, Tok::Comment);
                }
                b'\\' => {
                    self.pos += 1;
                    if self.pos < self.b.len() {
                        self.bump_char();
                    }
                    self.emit(start, Tok::Plain);
                    cmd_pos = false;
                }
                b'\'' => {
                    self.pos += 1;
                    while self.pos < self.b.len() && self.b[self.pos] != b'\'' {
                        self.pos += 1;
                    }
                    self.pos = (self.pos + 1).min(self.b.len());
                    self.emit(start, Tok::Str);
                    cmd_pos = false;
                }
                b'"' => {
                    self.dquote();
                    cmd_pos = false;
                }
                b'`' => {
                    self.pos += 1;
                    self.emit(start, Tok::Operator);
                    cmd_pos = true;
                }
                b'$' => {
                    if self.b[start..].starts_with(b"$((") {
                        self.arith(3);
                        cmd_pos = false;
                    } else if self.peek(1) == b'(' {
                        self.pos += 2;
                        self.emit(start, Tok::Operator);
                        depth += 1;
                        cmd_pos = true;
                    } else if let Some(n) = var_len(&self.b[start..]) {
                        self.pos += n;
                        self.emit(start, Tok::Var);
                        cmd_pos = false;
                    } else {
                        self.pos += 1;
                        self.emit(start, Tok::Plain);
                        cmd_pos = false;
                    }
                }
                b'(' => {
                    if cmd_pos && self.peek(1) == b'(' {
                        self.arith(2);
                        cmd_pos = false;
                    } else {
                        self.pos += 1;
                        self.emit(start, Tok::Operator);
                        depth += 1;
                        cmd_pos = true;
                    }
                }
                b')' => {
                    self.pos += 1;
                    self.emit(start, Tok::Operator);
                    if pattern_pos {
                        pattern_pos = false; // end of `pattern)`, a command follows
                        cmd_pos = true;
                    } else if depth == 0 {
                        if until_paren {
                            return;
                        }
                        cmd_pos = false;
                    } else {
                        depth -= 1;
                        cmd_pos = false;
                    }
                }
                b';' => {
                    self.pos += 1;
                    if matches!(self.peek(0), b';' | b'&') {
                        // `;;`  `;&`  `;;&` end a case branch: a pattern follows
                        while matches!(self.peek(0), b';' | b'&') {
                            self.pos += 1;
                        }
                        pattern_pos = case_depth > 0;
                    }
                    self.emit(start, Tok::Operator);
                    cmd_pos = true;
                    decl = false;
                }
                b'|' | b'&' => {
                    self.pos += 1;
                    self.emit(start, Tok::Operator);
                    if !pattern_pos {
                        cmd_pos = true;
                        decl = false;
                    }
                }
                b'<' | b'>' => {
                    if c == b'<' && self.peek(1) == b'<' && self.peek(2) != b'<' {
                        self.heredoc_start();
                    } else {
                        while matches!(self.peek(0), b'<' | b'>') {
                            self.pos += 1;
                        }
                        if self.peek(0) == b'&' {
                            // 2>&1, >&2, <&-
                            self.pos += 1;
                            while self.peek(0).is_ascii_digit() || self.peek(0) == b'-' {
                                self.pos += 1;
                            }
                        }
                        self.emit(start, Tok::Operator);
                    }
                }
                _ => {
                    while self.pos < self.b.len() && !is_meta(self.b[self.pos]) {
                        self.pos += 1;
                    }
                    let end = self.pos;
                    let word = &self.src[start..end];

                    if pattern_pos {
                        if word == "esac" {
                            self.emit(start, Tok::Keyword);
                            case_depth = case_depth.saturating_sub(1);
                            pattern_pos = false;
                            cmd_pos = false;
                        } else {
                            self.emit(start, Tok::Plain);
                        }
                    } else if let (true, Some((name, eq))) = (cmd_pos || decl, assignment(word)) {
                        self.pos = start + name;
                        self.emit(start, Tok::Var);
                        self.pos = start + eq + 1;
                        self.emit(start + name, Tok::Operator);
                        self.pos = end;
                        self.emit(start + eq + 1, Tok::Plain);
                        if eq + 1 == word.len() && self.peek(0) == b'(' {
                            // array=(a b c): the words inside are values, not commands
                            self.pos += 1;
                            self.emit(end, Tok::Operator);
                            depth += 1;
                            cmd_pos = false;
                        }
                        // otherwise cmd_pos stays as it is: `FOO=1 some-command`
                    } else if cmd_pos {
                        cmd_pos = false;
                        let tok = match word {
                            "if" | "then" | "else" | "elif" | "do" | "while" | "until" | "!"
                            | "time" => {
                                cmd_pos = true;
                                Tok::Keyword
                            }
                            "for" | "select" => {
                                loop_var = true;
                                Tok::Keyword
                            }
                            "case" => {
                                expect_case_in = true;
                                Tok::Keyword
                            }
                            "esac" => {
                                case_depth = case_depth.saturating_sub(1);
                                Tok::Keyword
                            }
                            "function" => {
                                cmd_pos = true; // the function name gets the command colour
                                Tok::Keyword
                            }
                            "local" | "export" | "declare" | "readonly" | "typeset" => {
                                decl = true;
                                Tok::Keyword
                            }
                            "fi" | "done" | "in" | "return" | "exit" | "break" | "continue" => {
                                Tok::Keyword
                            }
                            "{" => {
                                cmd_pos = true;
                                Tok::Operator
                            }
                            "}" | "[" | "[[" => Tok::Operator,
                            _ => Tok::Command,
                        };
                        self.emit(start, tok);
                    } else {
                        // an argument
                        let tok = if loop_var {
                            loop_var = false;
                            Tok::Var
                        } else if word == "in" {
                            if expect_case_in {
                                expect_case_in = false;
                                case_depth += 1;
                                pattern_pos = true;
                            }
                            Tok::Keyword
                        } else if word == "{" {
                            cmd_pos = true; // `name() {`
                            Tok::Operator
                        } else if matches!(word, "}" | "]" | "]]") {
                            Tok::Operator
                        } else if word.len() > 1 && word.starts_with('-') {
                            Tok::Param
                        } else if word.bytes().all(|c| c.is_ascii_digit()) {
                            Tok::Number
                        } else if decl {
                            Tok::Var
                        } else {
                            Tok::Plain
                        };
                        self.emit(start, tok);
                    }
                }
            }
        }
    }

    /// A "double quoted string", with `$variables` and `$(commands)` inside it.
    fn dquote(&mut self) {
        let mut seg = self.pos; // start of the current plain-string piece
        self.pos += 1;
        while self.pos < self.b.len() {
            match self.b[self.pos] {
                b'\\' => {
                    self.pos += 1;
                    if self.pos < self.b.len() {
                        self.bump_char();
                    }
                }
                b'"' => {
                    self.pos += 1;
                    self.emit(seg, Tok::Str);
                    return;
                }
                b'$' => {
                    let dollar = self.pos;
                    if self.b[dollar..].starts_with(b"$((") {
                        self.emit(seg, Tok::Str);
                        self.arith(3);
                        seg = self.pos;
                    } else if self.peek(1) == b'(' {
                        self.emit(seg, Tok::Str);
                        self.pos += 2;
                        self.emit(dollar, Tok::Operator);
                        self.run(true);
                        seg = self.pos;
                    } else if let Some(n) = var_len(&self.b[dollar..]) {
                        self.emit(seg, Tok::Str);
                        self.pos += n;
                        self.emit(dollar, Tok::Var);
                        seg = self.pos;
                    } else {
                        self.pos += 1;
                    }
                }
                _ => self.pos += 1,
            }
        }
        self.emit(seg, Tok::Str); // unterminated string: colour to the end
    }

    /// `$(( … ))` or `(( … ))`. `open` is the length of the opening bracket.
    fn arith(&mut self, open: usize) {
        let start = self.pos;
        self.pos += open;
        self.emit(start, Tok::Operator);
        let mut seg = self.pos;
        let mut depth = 0usize;
        while self.pos < self.b.len() {
            match self.b[self.pos] {
                b'\n' => break,
                b'(' => depth += 1,
                b')' if depth > 0 => depth -= 1,
                b')' => {
                    if self.peek(1) == b')' {
                        self.emit(seg, Tok::Plain);
                        let close = self.pos;
                        self.pos += 2;
                        self.emit(close, Tok::Operator);
                        return;
                    }
                    break;
                }
                b'$' => {
                    if let Some(n) = var_len(&self.b[self.pos..]) {
                        self.emit(seg, Tok::Plain);
                        let dollar = self.pos;
                        self.pos += n;
                        self.emit(dollar, Tok::Var);
                        seg = self.pos;
                        continue;
                    }
                }
                _ => {}
            }
            self.pos += 1;
        }
        self.emit(seg, Tok::Plain);
    }

    /// `<<EOF`, `<<-EOF`, `<<'EOF'`: remember the delimiter; the body starts on the next line.
    fn heredoc_start(&mut self) {
        let start = self.pos;
        self.pos += 2;
        let strip_tabs = self.peek(0) == b'-';
        if strip_tabs {
            self.pos += 1;
        }
        self.emit(start, Tok::Operator);

        let spaces = self.pos;
        while matches!(self.peek(0), b' ' | b'\t') {
            self.pos += 1;
        }
        self.emit(spaces, Tok::Plain);

        let delim_start = self.pos;
        let quote = self.peek(0);
        let delim = if quote == b'\'' || quote == b'"' {
            self.pos += 1;
            let from = self.pos;
            while self.pos < self.b.len() && !matches!(self.b[self.pos], b'\n') && self.b[self.pos] != quote {
                self.pos += 1;
            }
            let delim = &self.src[from..self.pos];
            if self.peek(0) == quote {
                self.pos += 1;
            }
            delim
        } else {
            if self.peek(0) == b'\\' {
                self.pos += 1;
            }
            let from = self.pos;
            while self.peek(0).is_ascii_alphanumeric() || self.peek(0) == b'_' {
                self.pos += 1;
            }
            &self.src[from..self.pos]
        };
        self.emit(delim_start, Tok::Str);

        // `1 << 2` is a bit shift, not a heredoc.
        if delim.bytes().next().is_some_and(|c| !c.is_ascii_digit()) {
            self.heredocs.push((delim.to_owned(), strip_tabs));
        }
    }

    /// Consume the bodies of all heredocs announced on the line we just finished.
    fn heredoc_bodies(&mut self) {
        for (delim, strip_tabs) in std::mem::take(&mut self.heredocs) {
            let start = self.pos;
            while self.pos < self.b.len() {
                let line_end = self.b[self.pos..]
                    .iter()
                    .position(|&c| c == b'\n')
                    .map_or(self.b.len(), |i| self.pos + i);
                let mut line = self.src[self.pos..line_end].trim_end_matches('\r');
                if strip_tabs {
                    line = line.trim_start_matches('\t');
                }
                self.pos = (line_end + 1).min(self.b.len());
                if line == delim {
                    break;
                }
            }
            self.emit(start, Tok::Str);
        }
    }
}

/// Turn a script into coloured text that egui can lay out.
fn highlight(src: &str, font: &FontId, line_height: f32, p: &Palette) -> LayoutJob {
    let mut job = LayoutJob::default();
    for (range, tok) in tokenize(src) {
        job.append(
            &src[range],
            0.0,
            TextFormat {
                font_id: font.clone(),
                color: p.token(tok),
                line_height: Some(line_height), // identical rows -> line numbers line up
                ..Default::default()
            },
        );
    }
    job.wrap.max_width = f32::INFINITY; // code is not wrapped; the editor scrolls sideways
    job.keep_trailing_whitespace = true;
    job
}

// ─────────────────────────────────────────────────────────────────────────────
// 3. Console buffer
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Out,  // stdout
    Err,  // stderr
    Cmd,  // the command line we echo before running something
    Info, // messages from the ISE itself
    Ok,   // "exit 0"
}

impl Kind {
    fn color(self) -> Color32 {
        match self {
            Kind::Out => CONSOLE_FG,
            Kind::Err => CONSOLE_ERR,
            Kind::Cmd => CONSOLE_PROMPT,
            Kind::Info => CONSOLE_DIM,
            Kind::Ok => CONSOLE_OK,
        }
    }
}

struct ConsoleLine {
    kind: Kind,
    text: String,
}

/// The text shown in the console pane, stored line by line so that only the
/// visible lines have to be drawn (important for scripts that print a lot).
#[derive(Default)]
struct Console {
    lines: Vec<ConsoleLine>,
    /// The last line has not received its '\n' yet (e.g. a `printf "Name: "` prompt).
    open: bool,
    /// We saw a '\r': the next text overwrites the line (progress bars).
    pending_cr: bool,
}

impl Console {
    const MAX_LINES: usize = 20_000;

    /// Append raw process output. It may contain any number of (partial) lines.
    fn push(&mut self, kind: Kind, text: &str) {
        for piece in text.split_inclusive('\n') {
            let ended = piece.ends_with('\n');
            let mut body = piece.strip_suffix('\n').unwrap_or(piece);
            if ended {
                body = body.strip_suffix('\r').unwrap_or(body); // Windows line ending
            }

            let continues = self.open && self.lines.last().is_some_and(|l| l.kind == kind);
            if !continues {
                self.lines.push(ConsoleLine { kind, text: String::new() });
                self.pending_cr = false;
            }
            let line = &mut self.lines.last_mut().expect("just pushed").text;

            for (i, segment) in body.split('\r').enumerate() {
                if i > 0 {
                    self.pending_cr = true;
                }
                if !segment.is_empty() {
                    if self.pending_cr {
                        line.clear();
                        self.pending_cr = false;
                    }
                    line.push_str(segment);
                }
            }
            if ended {
                self.pending_cr = false;
            }
            self.open = !ended;
        }

        // Keep memory bounded: drop the oldest lines in batches.
        if self.lines.len() > Self::MAX_LINES + 2_000 {
            let excess = self.lines.len() - Self::MAX_LINES;
            self.lines.drain(..excess);
        }
    }

    /// Append one complete line (ends an unfinished line first).
    fn line(&mut self, kind: Kind, text: &str) {
        self.open = false;
        self.push(kind, text);
        self.open = false;
    }

    /// Echo what the user typed for a running script. If the script printed a
    /// prompt without a newline ("Name: "), the answer continues that line.
    fn echo_input(&mut self, text: &str) {
        let kind = match (self.open, self.lines.last()) {
            (true, Some(prompt)) => prompt.kind,
            _ => Kind::Info,
        };
        self.push(kind, &format!("{text}\n"));
    }

    fn clear(&mut self) {
        *self = Self::default();
    }

    fn to_text(&self) -> String {
        let mut text = String::new();
        for line in &self.lines {
            text.push_str(&line.text);
            text.push('\n');
        }
        text
    }
}

/// Remove ANSI escape sequences (colours, cursor movement) that scripts print for terminals.
fn strip_ansi(s: &str) -> Cow<'_, str> {
    if !s.contains('\x1b') {
        return Cow::Borrowed(s); // the common case: nothing to remove, nothing to copy
    }
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\x1b' {
            out.push(c);
            continue;
        }
        match chars.peek() {
            // CSI: ESC [ parameters… final-letter
            Some('[') => {
                chars.next();
                for c in chars.by_ref() {
                    if ('\x40'..='\x7e').contains(&c) {
                        break;
                    }
                }
            }
            // OSC: ESC ] … BEL   or   ESC ] … ESC \
            Some(']') => {
                chars.next();
                while let Some(c) = chars.next() {
                    if c == '\x07' {
                        break;
                    }
                    if c == '\x1b' {
                        chars.next();
                        break;
                    }
                }
            }
            // Two-character sequences such as ESC ( B
            Some(_) => {
                chars.next();
            }
            None => {}
        }
    }
    Cow::Owned(out)
}

// ─────────────────────────────────────────────────────────────────────────────
// 4. Runner
// ─────────────────────────────────────────────────────────────────────────────

/// What the background threads send to the UI.
enum Msg {
    Out(String),
    Err(String),
    Exit { code: Option<i32>, signal: Option<i32> },
}

/// A bash process that is currently running.
struct Job {
    rx: Receiver<Msg>,
    /// Process id of bash. It is also the id of its process *group*, which is
    /// what we signal on Stop so that children (sleep, curl, …) die too.
    pid: u32,
    /// Lets the console input feed the script's stdin. `None` after Ctrl+D.
    stdin: Option<ChildStdin>,
    started: Instant,
    /// Console commands write their final working directory here, so `cd` sticks.
    cwd_file: Option<PathBuf>,
    /// Untitled scripts are run from a temporary file, removed afterwards.
    temp_script: Option<PathBuf>,
    stop_requested: bool,
}

/// Start `cmd` and stream its output through a channel.
///
/// Three threads are involved: one reads stdout, one reads stderr and one waits
/// for the process to finish. The UI never blocks; it just drains the channel.
fn spawn(mut cmd: Command, ctx: &egui::Context) -> std::io::Result<(u32, Option<ChildStdin>, Receiver<Msg>)> {
    cmd.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0); // own process group -> Stop can signal the whole tree
    let mut child = cmd.spawn()?;
    let pid = child.id();
    let stdin = child.stdin.take();
    let stdout = child.stdout.take().expect("stdout is piped");
    let stderr = child.stderr.take().expect("stderr is piped");

    // A *bounded* channel: if a script prints faster than the UI can draw,
    // the reader threads wait instead of filling up memory.
    let (tx, rx) = sync_channel(64);
    let readers = [
        pump(stdout, tx.clone(), ctx.clone(), Msg::Out),
        pump(stderr, tx.clone(), ctx.clone(), Msg::Err),
    ];

    let ctx = ctx.clone();
    thread::spawn(move || {
        let status = child.wait();
        for reader in readers {
            let _ = reader.join(); // make sure all output arrives before "Exit"
        }
        let (code, signal) = match status {
            Ok(status) => (status.code(), status.signal()),
            Err(_) => (None, None),
        };
        let _ = tx.send(Msg::Exit { code, signal });
        ctx.request_repaint(); // wake the UI up
    });

    Ok((pid, stdin, rx))
}

/// Read a pipe in chunks (not lines, so prompts without '\n' show up at once).
fn pump(
    mut pipe: impl Read + Send + 'static,
    tx: SyncSender<Msg>,
    ctx: egui::Context,
    wrap: fn(String) -> Msg,
) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        let mut buf = [0u8; 8192];
        let mut pending: Vec<u8> = Vec::new();
        loop {
            match pipe.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    pending.extend_from_slice(&buf[..n]);
                    let text = take_utf8(&mut pending);
                    if !text.is_empty() {
                        if tx.send(wrap(text)).is_err() {
                            return; // the UI is gone
                        }
                        ctx.request_repaint();
                    }
                }
            }
        }
        if !pending.is_empty() {
            let _ = tx.send(wrap(String::from_utf8_lossy(&pending).into_owned()));
        }
    })
}

/// Take all complete UTF-8 text out of `pending`. A character that was cut in
/// half by the chunk boundary stays behind until the rest of it arrives.
fn take_utf8(pending: &mut Vec<u8>) -> String {
    match std::str::from_utf8(pending) {
        Ok(text) => {
            let text = text.to_owned();
            pending.clear();
            text
        }
        Err(e) if e.error_len().is_none() => {
            let rest = pending.split_off(e.valid_up_to());
            let text = String::from_utf8_lossy(pending).into_owned();
            *pending = rest;
            text
        }
        Err(_) => {
            // Not UTF-8 at all (binary output): show replacement characters.
            let text = String::from_utf8_lossy(pending).into_owned();
            pending.clear();
            text
        }
    }
}

/// Send a signal to a whole process group.
fn signal_group(pgid: u32, signal: libc::c_int) {
    // SAFETY: `kill` only takes plain integers; a negative pid means "the
    // process group with that id". The worst case is an error return value.
    unsafe {
        libc::kill(-(pgid as libc::pid_t), signal);
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// 5. Tabs
// ─────────────────────────────────────────────────────────────────────────────

const NEW_SCRIPT: &str = "#!/usr/bin/env bash\n\n";

struct Tab {
    /// Stable id for egui (the index changes when tabs are closed).
    id: u64,
    name: String,
    path: Option<PathBuf>,
    text: String,
    dirty: bool,
    /// Selection as sorted *character* indices. Equal values = just a cursor.
    sel: (usize, usize),
    /// 1-based line and column of the cursor, for the status bar.
    line_col: (usize, usize),
    /// Cursor position `line_col` was computed for (to avoid recounting every frame).
    line_col_at: usize,
    /// Selection to apply on the next frame (after open, find, snippet insert).
    pending_select: Option<(usize, usize)>,
    /// Current search match, highlighted while the find bar is open.
    find_hit: Option<(usize, usize)>,
    /// Cached highlighting, reused as long as text and theme stay the same.
    highlighted: Option<Highlighted>,
}

struct Highlighted {
    text: String,
    dark: bool,
    job: LayoutJob,
}

impl Tab {
    fn new(id: u64, name: String, path: Option<PathBuf>, text: String) -> Self {
        let end = text.chars().count();
        Self {
            id,
            name,
            path,
            text,
            dirty: false,
            sel: (end, end),
            line_col: (1, 1),
            line_col_at: usize::MAX,
            pending_select: Some((end, end)),
            find_hit: None,
            highlighted: None,
        }
    }

    fn editor_id(&self) -> egui::Id {
        egui::Id::unique(("bash-ise editor", self.id))
    }

    /// The selected text, or the line the cursor is on if nothing is selected.
    fn selection_or_line(&self) -> &str {
        let (a, b) = self.sel;
        let (a, b) = (byte_index(&self.text, a), byte_index(&self.text, b));
        if a != b {
            return &self.text[a..b];
        }
        let start = self.text[..a].rfind('\n').map_or(0, |i| i + 1);
        let end = self.text[a..].find('\n').map_or(self.text.len(), |i| a + i);
        &self.text[start..end]
    }
}

/// egui counts characters, Rust strings are indexed by bytes: convert.
fn byte_index(s: &str, char_index: usize) -> usize {
    s.char_indices().nth(char_index).map_or(s.len(), |(i, _)| i)
}

fn char_index(s: &str, byte_index: usize) -> usize {
    s[..byte_index].chars().count()
}

/// "/home/me/scripts" -> "~/scripts"
fn pretty_path(path: &Path) -> String {
    let home = std::env::var_os("HOME").unwrap_or_default();
    match path.strip_prefix(&home) {
        Ok(rest) if !home.is_empty() && rest.as_os_str().is_empty() => "~".to_owned(),
        Ok(rest) if !home.is_empty() => format!("~/{}", rest.display()),
        _ => path.display().to_string(),
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// 6. The application
// ─────────────────────────────────────────────────────────────────────────────

const SC_NEW: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::N);
const SC_OPEN: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::O);
const SC_SAVE: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::S);
const SC_SAVE_AS: KeyboardShortcut =
    KeyboardShortcut::new(Modifiers::COMMAND.plus(Modifiers::SHIFT), Key::S);
const SC_CLOSE: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::W);
const SC_QUIT: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::Q);
const SC_FIND: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::F);
const SC_FIND_NEXT: KeyboardShortcut = KeyboardShortcut::new(Modifiers::NONE, Key::F3);
const SC_FIND_PREV: KeyboardShortcut = KeyboardShortcut::new(Modifiers::SHIFT, Key::F3);
const SC_RUN: KeyboardShortcut = KeyboardShortcut::new(Modifiers::NONE, Key::F5);
const SC_RUN_SELECTION: KeyboardShortcut = KeyboardShortcut::new(Modifiers::NONE, Key::F8);
const SC_STOP: KeyboardShortcut = KeyboardShortcut::new(Modifiers::SHIFT, Key::F5);
const SC_CLEAR: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::L);

/// (menu label, text that gets inserted)
const SNIPPETS: &[(&str, &str)] = &[
    ("Strict mode", "set -euo pipefail\n"),
    ("if", "if [[ condition ]]; then\n    \nfi\n"),
    ("if / else", "if [[ condition ]]; then\n    \nelse\n    \nfi\n"),
    ("for … in", "for item in \"${items[@]}\"; do\n    echo \"$item\"\ndone\n"),
    ("for (counter)", "for ((i = 0; i < 10; i++)); do\n    echo \"$i\"\ndone\n"),
    ("while read line", "while IFS= read -r line; do\n    echo \"$line\"\ndone < \"$file\"\n"),
    ("case", "case \"$1\" in\n    start)\n        ;;\n    stop)\n        ;;\n    *)\n        echo \"Usage: $0 {start|stop}\" >&2\n        exit 1\n        ;;\nesac\n"),
    ("function", "my_function() {\n    local arg=\"$1\"\n    \n}\n"),
    ("Script directory", "script_dir=\"$(cd \"$(dirname \"${BASH_SOURCE[0]}\")\" && pwd)\"\n"),
];

/// Everything the user can ask for. The UI only *collects* actions; they are
/// applied in one place afterwards, which keeps the borrow checker happy
/// (the UI code borrows `self`, the actions change it).
#[derive(Clone, Copy)]
enum Action {
    New,
    Open,
    Save,
    SaveAs,
    SelectTab(usize),
    CloseTab(usize),
    Quit,
    Run,
    RunSelection,
    Stop,
    ClearConsole,
    CopyConsole,
    ToggleFind,
    FindNext,
    FindPrev,
    Snippet(&'static str),
}

/// A pending "you have unsaved changes" question.
#[derive(Clone, Copy)]
enum Confirm {
    CloseTab(usize),
    Quit,
}

#[derive(Default)]
struct Find {
    open: bool,
    query: String,
    match_case: bool,
    not_found: bool,
    grab_focus: bool,
}

struct BashIse {
    tabs: Vec<Tab>,
    active: usize,
    next_tab_id: u64,
    untitled_count: usize,

    console: Console,
    show_console: bool,
    job: Option<Job>,
    /// Text for the status bar about the last finished run.
    last_result: Option<(bool, String)>,
    /// Working directory for everything we run; `cd` in the console changes it.
    cwd: PathBuf,
    /// Console input line and its history (Up/Down).
    input: String,
    history: Vec<String>,
    history_pos: Option<usize>,
    focus_input: bool,
    /// Arguments passed to the script on F5.
    args: String,

    find: Find,
    confirm: Option<Confirm>,
    allow_close: bool,
    window_title: String,
}

impl BashIse {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        setup_style(&cc.egui_ctx);

        let mut app = Self {
            tabs: Vec::new(),
            active: 0,
            next_tab_id: 0,
            untitled_count: 0,
            console: Console::default(),
            show_console: true,
            job: None,
            last_result: None,
            cwd: std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/")),
            input: String::new(),
            history: Vec::new(),
            history_pos: None,
            focus_input: false,
            args: String::new(),
            find: Find::default(),
            confirm: None,
            allow_close: false,
            window_title: String::new(),
        };

        // Say hello with the bash version (and find out early if bash is missing).
        match Command::new("bash").arg("--version").output() {
            Ok(out) => {
                let version = String::from_utf8_lossy(&out.stdout);
                app.console.line(Kind::Info, version.lines().next().unwrap_or("bash"));
                app.console.line(
                    Kind::Info,
                    "F5 runs the script, F8 runs the selection or current line. Type commands below.",
                );
            }
            Err(e) => app.console.line(Kind::Err, &format!("bash was not found: {e}")),
        }

        // `bash-ise script.sh other.sh` opens those files.
        for arg in std::env::args_os().skip(1) {
            app.open_path(&PathBuf::from(arg));
        }
        if app.tabs.is_empty() {
            app.new_tab();
        }
        app
    }

    // ── tabs & files ────────────────────────────────────────────────────────

    fn new_tab(&mut self) {
        self.untitled_count += 1;
        let name = format!("Untitled{}.sh", self.untitled_count);
        self.push_tab(name, None, NEW_SCRIPT.to_owned());
    }

    fn push_tab(&mut self, name: String, path: Option<PathBuf>, text: String) {
        self.next_tab_id += 1;
        self.tabs.push(Tab::new(self.next_tab_id, name, path, text));
        self.active = self.tabs.len() - 1;
    }

    fn open_dialog(&mut self) {
        let dir = self.dialog_dir();
        let picked = rfd::FileDialog::new()
            .set_title("Open script")
            .set_directory(dir)
            .add_filter("Shell scripts", &["sh", "bash", "bashrc", "profile", "env"])
            .add_filter("All files", &["*"])
            .pick_files();
        for path in picked.unwrap_or_default() {
            self.open_path(&path);
        }
    }

    fn open_path(&mut self, path: &Path) {
        // Resolve relative paths and symlinks so the same file is not opened twice.
        let path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        if let Some(i) = self.tabs.iter().position(|t| t.path.as_deref() == Some(&*path)) {
            self.active = i;
            return;
        }
        let text = match std::fs::read(&path) {
            Ok(bytes) => String::from_utf8_lossy(&bytes).replace("\r\n", "\n"),
            Err(e) => {
                self.console.line(Kind::Err, &format!("Cannot open {}: {e}", path.display()));
                return;
            }
        };
        let name = file_name(&path);

        // Reuse the empty "Untitled" tab the app starts with.
        let reuse = self.tabs.len() == 1 && {
            let t = &self.tabs[0];
            t.path.is_none() && !t.dirty && t.text == NEW_SCRIPT
        };
        if reuse {
            self.tabs.clear();
        }
        self.push_tab(name, Some(path), text);
        // Start at the top of an opened file.
        self.tabs[self.active].pending_select = Some((0, 0));
        self.tabs[self.active].sel = (0, 0);
    }

    /// Save tab `i`. Returns false if the user cancelled or writing failed.
    fn save_tab(&mut self, i: usize, ask_for_path: bool) -> bool {
        let path = match (&self.tabs[i].path, ask_for_path) {
            (Some(path), false) => path.clone(),
            _ => {
                let picked = rfd::FileDialog::new()
                    .set_title("Save script")
                    .set_directory(self.dialog_dir())
                    .set_file_name(&self.tabs[i].name)
                    .add_filter("Shell scripts", &["sh", "bash"])
                    .add_filter("All files", &["*"])
                    .save_file();
                match picked {
                    Some(path) => path,
                    None => return false,
                }
            }
        };
        match std::fs::write(&path, &self.tabs[i].text) {
            Ok(()) => {
                let tab = &mut self.tabs[i];
                tab.name = file_name(&path);
                tab.path = Some(path);
                tab.dirty = false;
                true
            }
            Err(e) => {
                self.console.line(Kind::Err, &format!("Cannot save {}: {e}", path.display()));
                false
            }
        }
    }

    /// Folder the file dialogs start in: next to the current file, else the working directory.
    fn dialog_dir(&self) -> PathBuf {
        self.tabs
            .get(self.active)
            .and_then(|t| t.path.as_deref())
            .and_then(Path::parent)
            .map_or_else(|| self.cwd.clone(), Path::to_path_buf)
    }

    fn remove_tab(&mut self, i: usize) {
        self.tabs.remove(i);
        if self.tabs.is_empty() {
            self.new_tab();
        } else if self.active > i || self.active >= self.tabs.len() {
            self.active = self.active.saturating_sub(1);
        }
    }

    // ── running things ──────────────────────────────────────────────────────

    /// F5: run the whole script, like `bash script.sh args…` in a terminal.
    fn run_script(&mut self, ctx: &egui::Context) {
        if self.job.is_some() {
            return;
        }
        let i = self.active;
        let mut temp_script = None;

        let path = if self.tabs[i].path.is_some() {
            // Like the ISE: running a saved file saves it first.
            if self.tabs[i].dirty && !self.save_tab(i, false) {
                return;
            }
            self.tabs[i].path.clone().expect("checked above")
        } else {
            // Untitled: run a temporary copy.
            let tab = &self.tabs[i];
            let tmp = std::env::temp_dir()
                .join(format!("bash-ise-{}-{}.sh", std::process::id(), tab.id));
            if let Err(e) = std::fs::write(&tmp, &tab.text) {
                self.console.line(Kind::Err, &format!("Cannot write {}: {e}", tmp.display()));
                return;
            }
            temp_script = Some(tmp.clone());
            tmp
        };

        let args = self.args.trim().to_owned();
        let mut cmd = Command::new("bash");
        if args.is_empty() {
            cmd.arg(&path);
        } else {
            // Let bash itself split the arguments, so quotes and $VARS work
            // exactly as they would on a command line. "$0" is the script path.
            cmd.arg("-c").arg(format!("exec bash \"$0\" {args}")).arg(&path);
        }

        let shown = if temp_script.is_some() {
            self.tabs[i].name.clone()
        } else {
            pretty_path(&path)
        };
        let echo = format!("bash {shown} {args}");
        self.start(cmd, echo.trim_end(), None, temp_script, ctx);
    }

    /// F8 and the console input: run a piece of bash code.
    fn run_code(&mut self, code: &str, ctx: &egui::Context) {
        if self.job.is_some() || code.trim().is_empty() {
            return;
        }
        // Every run is a fresh bash process, so a `cd` would normally be lost.
        // The EXIT trap reports the final directory back to us so `cd` sticks.
        let cwd_file = std::env::temp_dir().join(format!("bash-ise-{}.cwd", std::process::id()));
        let mut cmd = Command::new("bash");
        cmd.arg("-c")
            .arg(format!("trap 'pwd > \"$BASH_ISE_CWD_FILE\"' EXIT; {code}"))
            .env("BASH_ISE_CWD_FILE", &cwd_file);

        let mut lines = code.trim_end().lines();
        let first = lines.next().unwrap_or_default();
        let more = lines.count();
        let echo = if more == 0 {
            first.to_owned()
        } else {
            format!("{first}  … (+{more} more lines)")
        };
        self.start(cmd, &echo, Some(cwd_file), None, ctx);
    }

    fn start(
        &mut self,
        mut cmd: Command,
        echo: &str,
        cwd_file: Option<PathBuf>,
        temp_script: Option<PathBuf>,
        ctx: &egui::Context,
    ) {
        self.show_console = true;
        self.last_result = None;
        self.console.line(Kind::Cmd, &format!("{} $ {echo}", pretty_path(&self.cwd)));
        cmd.current_dir(&self.cwd);
        match spawn(cmd, ctx) {
            Ok((pid, stdin, rx)) => {
                self.job = Some(Job {
                    rx,
                    pid,
                    stdin,
                    started: Instant::now(),
                    cwd_file,
                    temp_script,
                    stop_requested: false,
                });
            }
            Err(e) => self.console.line(Kind::Err, &format!("Failed to start bash: {e}")),
        }
    }

    /// First Stop asks politely (SIGTERM), a second one kills (SIGKILL).
    fn stop(&mut self) {
        if let Some(job) = &mut self.job {
            let signal = if job.stop_requested { libc::SIGKILL } else { libc::SIGTERM };
            signal_group(job.pid, signal);
            job.stop_requested = true;
        }
    }

    /// Move output from the background threads into the console. Called every frame.
    fn poll_job(&mut self, ctx: &egui::Context) {
        let Some(job) = &mut self.job else { return };

        // Spend at most a few milliseconds per frame here so the UI stays
        // responsive even with `yes`-style output floods.
        let deadline = Instant::now() + Duration::from_millis(8);
        let mut exit = None;
        loop {
            match job.rx.try_recv() {
                Ok(Msg::Out(text)) => self.console.push(Kind::Out, &strip_ansi(&text)),
                Ok(Msg::Err(text)) => self.console.push(Kind::Err, &strip_ansi(&text)),
                Ok(Msg::Exit { code, signal }) => {
                    exit = Some((code, signal));
                    break;
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    exit = Some((None, None));
                    break;
                }
            }
            if Instant::now() > deadline {
                ctx.request_repaint();
                break;
            }
        }

        let Some((code, signal)) = exit else {
            // Still running: repaint now and then so the timer keeps ticking.
            ctx.request_repaint_after(Duration::from_millis(100));
            return;
        };

        let job = self.job.take().expect("checked above");
        let secs = job.started.elapsed().as_secs_f32();
        if let Some(file) = &job.cwd_file {
            if let Ok(dir) = std::fs::read_to_string(file) {
                let dir = PathBuf::from(dir.trim_end_matches('\n'));
                if dir.is_dir() {
                    self.cwd = dir;
                }
            }
            let _ = std::fs::remove_file(file);
        }
        if let Some(file) = &job.temp_script {
            let _ = std::fs::remove_file(file);
        }

        let (ok, summary) = match (code, signal) {
            (Some(0), _) => (true, format!("exit 0 · {secs:.2}s")),
            (Some(code), _) => (false, format!("exit {code} · {secs:.2}s")),
            (None, Some(signal)) if job.stop_requested => {
                (false, format!("stopped (signal {signal}) · {secs:.2}s"))
            }
            (None, Some(signal)) => (false, format!("killed by signal {signal} · {secs:.2}s")),
            (None, None) => (false, "finished (unknown status)".to_owned()),
        };
        self.console.line(if ok { Kind::Ok } else { Kind::Err }, &format!("[{summary}]"));
        self.last_result = Some((ok, summary));
    }

    /// Enter in the console input.
    fn submit_input(&mut self, ctx: &egui::Context) {
        let line = std::mem::take(&mut self.input);
        self.history_pos = None;

        if let Some(job) = &mut self.job {
            // A script is running: the line goes to its stdin (answers `read`).
            self.console.echo_input(&line);
            if let Some(stdin) = &mut job.stdin {
                let _ = stdin.write_all(format!("{line}\n").as_bytes());
                let _ = stdin.flush();
            }
            return;
        }

        if line.trim().is_empty() {
            return;
        }
        if self.history.last() != Some(&line) {
            self.history.push(line.clone());
        }
        match line.trim() {
            "clear" | "cls" => self.console.clear(),
            _ => self.run_code(&line, ctx),
        }
    }

    // ── find ────────────────────────────────────────────────────────────────

    fn find(&mut self, forward: bool) {
        let tab = &mut self.tabs[self.active];
        let query = &self.find.query;
        if query.is_empty() {
            tab.find_hit = None;
            self.find.not_found = false;
            return;
        }
        // ASCII lower-casing keeps byte offsets identical, so positions found
        // in the lower-cased copy are valid in the original text too.
        let (hay, needle) = if self.find.match_case {
            (tab.text.clone(), query.clone())
        } else {
            (tab.text.to_ascii_lowercase(), query.to_ascii_lowercase())
        };
        // Continue from the previous match (or from the cursor).
        let (from, to) = tab.find_hit.unwrap_or(tab.sel);
        let (from, to) = (byte_index(&hay, from), byte_index(&hay, to));
        let hit = if forward {
            hay[to..].find(&needle).map(|i| i + to).or_else(|| hay.find(&needle))
        } else {
            hay[..from].rfind(&needle).or_else(|| hay.rfind(&needle))
        };
        self.find.not_found = hit.is_none();
        tab.find_hit = hit.map(|byte| {
            let start = char_index(&hay, byte);
            (start, start + needle.chars().count())
        });
    }

    fn close_find(&mut self) {
        self.find.open = false;
        self.find.not_found = false;
        // Leave the match selected in the editor, ready to type over.
        let tab = &mut self.tabs[self.active];
        tab.pending_select = Some(tab.find_hit.take().unwrap_or(tab.sel));
    }

    // ── actions ─────────────────────────────────────────────────────────────

    fn apply(&mut self, action: Action, ctx: &egui::Context) {
        match action {
            Action::New => self.new_tab(),
            Action::Open => self.open_dialog(),
            Action::Save => {
                self.save_tab(self.active, false);
            }
            Action::SaveAs => {
                self.save_tab(self.active, true);
            }
            Action::SelectTab(i) => {
                if i < self.tabs.len() && i != self.active {
                    self.active = i;
                    // Put the keyboard focus (and cursor) back where it was.
                    let tab = &mut self.tabs[i];
                    tab.pending_select = Some(tab.sel);
                }
            }
            Action::CloseTab(i) => {
                if i >= self.tabs.len() {
                    return;
                }
                if self.tabs[i].dirty {
                    self.active = i;
                    self.confirm = Some(Confirm::CloseTab(i));
                } else {
                    self.remove_tab(i);
                }
            }
            Action::Quit => {
                if self.tabs.iter().any(|t| t.dirty) {
                    self.confirm = Some(Confirm::Quit);
                } else {
                    self.allow_close = true;
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
            }
            Action::Run => self.run_script(ctx),
            Action::RunSelection => {
                let code = self.tabs[self.active].selection_or_line().to_owned();
                self.run_code(&code, ctx);
            }
            Action::Stop => self.stop(),
            Action::ClearConsole => self.console.clear(),
            Action::CopyConsole => ctx.copy_text(self.console.to_text()),
            Action::ToggleFind => {
                if self.find.open {
                    self.close_find();
                } else {
                    self.find.open = true;
                    self.find.grab_focus = true;
                    // Pre-fill with the selection if it is a single line.
                    let tab = &self.tabs[self.active];
                    if tab.sel.0 != tab.sel.1 {
                        let selected = tab.selection_or_line();
                        if !selected.contains('\n') {
                            self.find.query = selected.to_owned();
                        }
                    }
                }
            }
            Action::FindNext | Action::FindPrev => {
                if self.find.open {
                    self.find(matches!(action, Action::FindNext));
                } else {
                    self.find.open = true;
                    self.find.grab_focus = true;
                }
            }
            Action::Snippet(snippet) => {
                let tab = &mut self.tabs[self.active];
                let (a, b) = tab.sel;
                let range = byte_index(&tab.text, a)..byte_index(&tab.text, b);
                tab.text.replace_range(range, snippet);
                tab.dirty = true;
                let end = a + snippet.chars().count();
                tab.pending_select = Some((end, end));
            }
        }
    }

    /// Answer to the "unsaved changes" dialog.
    fn resolve_confirm(&mut self, confirm: Confirm, save: bool, ctx: &egui::Context) {
        match confirm {
            Confirm::CloseTab(i) => {
                if i < self.tabs.len() && (!save || self.save_tab(i, false)) {
                    self.remove_tab(i);
                }
            }
            Confirm::Quit => {
                if save {
                    for i in 0..self.tabs.len() {
                        if self.tabs[i].dirty {
                            self.active = i;
                            if !self.save_tab(i, false) {
                                return; // cancelled: stay open
                            }
                        }
                    }
                }
                self.allow_close = true;
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
        }
    }

    // ── UI pieces ───────────────────────────────────────────────────────────

    fn shortcuts(&mut self, ctx: &egui::Context, actions: &mut Vec<Action>) {
        // More specific shortcuts first: Ctrl+Shift+S must win over Ctrl+S.
        let table = [
            (SC_SAVE_AS, Action::SaveAs),
            (SC_SAVE, Action::Save),
            (SC_NEW, Action::New),
            (SC_OPEN, Action::Open),
            (SC_CLOSE, Action::CloseTab(self.active)),
            (SC_QUIT, Action::Quit),
            (SC_FIND, Action::ToggleFind),
            (SC_FIND_PREV, Action::FindPrev),
            (SC_FIND_NEXT, Action::FindNext),
            (SC_STOP, Action::Stop),
            (SC_RUN, Action::Run),
            (SC_RUN_SELECTION, Action::RunSelection),
            (SC_CLEAR, Action::ClearConsole),
        ];
        ctx.input_mut(|input| {
            for (shortcut, action) in table {
                if input.consume_shortcut(&shortcut) {
                    actions.push(action);
                }
            }
        });
    }

    fn menu_bar(&mut self, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
        let running = self.job.is_some();
        egui::MenuBar::new().ui(ui, |ui| {
            ui.menu_button("File", |ui| {
                menu_item(ui, "New", Some(SC_NEW), true, Action::New, actions);
                menu_item(ui, "Open…", Some(SC_OPEN), true, Action::Open, actions);
                ui.separator();
                menu_item(ui, "Save", Some(SC_SAVE), true, Action::Save, actions);
                menu_item(ui, "Save As…", Some(SC_SAVE_AS), true, Action::SaveAs, actions);
                ui.separator();
                let close = Action::CloseTab(self.active);
                menu_item(ui, "Close Tab", Some(SC_CLOSE), true, close, actions);
                menu_item(ui, "Quit", Some(SC_QUIT), true, Action::Quit, actions);
            });
            ui.menu_button("Edit", |ui| {
                menu_item(ui, "Find…", Some(SC_FIND), true, Action::ToggleFind, actions);
                menu_item(ui, "Find Next", Some(SC_FIND_NEXT), true, Action::FindNext, actions);
                menu_item(ui, "Find Previous", Some(SC_FIND_PREV), true, Action::FindPrev, actions);
                ui.separator();
                ui.menu_button("Insert Snippet", |ui| {
                    for (label, snippet) in SNIPPETS {
                        menu_item(ui, label, None, true, Action::Snippet(snippet), actions);
                    }
                });
            });
            ui.menu_button("Run", |ui| {
                menu_item(ui, "Run Script", Some(SC_RUN), !running, Action::Run, actions);
                let run_selection = Action::RunSelection;
                menu_item(ui, "Run Selection / Line", Some(SC_RUN_SELECTION), !running, run_selection, actions);
                menu_item(ui, "Stop", Some(SC_STOP), running, Action::Stop, actions);
                ui.separator();
                menu_item(ui, "Clear Console", Some(SC_CLEAR), true, Action::ClearConsole, actions);
            });
            ui.menu_button("View", |ui| {
                ui.checkbox(&mut self.show_console, "Console pane");
                ui.separator();
                ui.label("Theme");
                egui::widgets::global_theme_preference_buttons(ui);
                ui.separator();
                egui::gui_zoom::zoom_menu_buttons(ui);
            });
        });
    }

    fn toolbar(&mut self, ui: &mut egui::Ui, p: &Palette, actions: &mut Vec<Action>) {
        let running = self.job.is_some();
        ui.horizontal(|ui| {
            tool_button(ui, "New", "New script", SC_NEW, true, Action::New, actions);
            tool_button(ui, "Open", "Open a script", SC_OPEN, true, Action::Open, actions);
            let dirty = self.tabs[self.active].dirty;
            tool_button(ui, "Save", "Save", SC_SAVE, dirty, Action::Save, actions);
            ui.separator();

            let run = RichText::new("▶ Run").color(p.run).strong();
            tool_button(ui, run, "Run the script", SC_RUN, !running, Action::Run, actions);
            tool_button(
                ui,
                "▶ Selection",
                "Run the selection, or the current line",
                SC_RUN_SELECTION,
                !running,
                Action::RunSelection,
                actions,
            );
            let stop = RichText::new("■ Stop").color(p.stop).strong();
            tool_button(ui, stop, "Stop (press twice to force-kill)", SC_STOP, running, Action::Stop, actions);
            ui.separator();

            ui.label("Arguments");
            ui.add(
                egui::TextEdit::singleline(&mut self.args)
                    .font(egui::TextStyle::Monospace)
                    .hint_text("passed to the script on Run")
                    .desired_width(260.0),
            );
        });
    }

    fn tab_bar(&mut self, ui: &mut egui::Ui, p: &Palette, actions: &mut Vec<Action>) {
        egui::ScrollArea::horizontal().id_salt("tabs").show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 2.0;
                for (i, tab) in self.tabs.iter().enumerate() {
                    let selected = i == self.active;
                    let frame = egui::Frame::new()
                        .fill(if selected { p.editor_bg } else { Color32::TRANSPARENT })
                        .corner_radius(egui::CornerRadius { nw: 6, ne: 6, sw: 0, se: 0 })
                        .inner_margin(Margin { left: 12, right: 6, top: 6, bottom: 6 });
                    let shown = frame.show(ui, |ui| {
                        ui.spacing_mut().item_spacing.x = 6.0;
                        let mut title = RichText::new(&tab.name);
                        if selected {
                            title = title.color(ui.visuals().strong_text_color());
                        }
                        let label = egui::Label::new(title).selectable(false).sense(egui::Sense::click());
                        let label = ui.add(label);
                        let label = match &tab.path {
                            Some(path) => label.on_hover_text(path.display().to_string()),
                            None => label.on_hover_text("Not saved yet"),
                        };
                        if label.clicked() {
                            actions.push(Action::SelectTab(i));
                        }
                        if label.middle_clicked() {
                            actions.push(Action::CloseTab(i));
                        }
                        // A dot while there are unsaved changes, otherwise the close cross.
                        let close = if tab.dirty { "●" } else { "×" };
                        let close = egui::Button::new(close).frame_when_inactive(false).small();
                        if ui.add(close).on_hover_text("Close").clicked() {
                            actions.push(Action::CloseTab(i));
                        }
                    });
                    if selected {
                        // Accent line on top of the active tab.
                        let rect = shown.response.rect;
                        let line = egui::Rect::from_min_max(rect.left_top(), rect.right_top() + egui::vec2(0.0, 2.0));
                        ui.painter().rect_filled(line, 2.0, p.accent);
                    }
                }
                ui.add_space(4.0);
                let plus = egui::Button::new("+").frame_when_inactive(false);
                if ui.add(plus).on_hover_text("New script").clicked() {
                    actions.push(Action::New);
                }
            });
        });
    }

    fn find_bar(&mut self, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
        ui.horizontal(|ui| {
            ui.label("Find");
            let field = ui.add(
                egui::TextEdit::singleline(&mut self.find.query)
                    .desired_width(260.0)
                    .hint_text("text to search for"),
            );
            if std::mem::take(&mut self.find.grab_focus) {
                field.request_focus();
            }
            if field.changed() {
                // Search as you type, starting again from the cursor.
                self.tabs[self.active].find_hit = None;
                self.find(true);
            }
            if field.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                let back = ui.input(|i| i.modifiers.shift);
                actions.push(if back { Action::FindPrev } else { Action::FindNext });
                field.request_focus();
            }
            if ui.button("Previous").clicked() {
                actions.push(Action::FindPrev);
            }
            if ui.button("Next").clicked() {
                actions.push(Action::FindNext);
            }
            if ui.checkbox(&mut self.find.match_case, "Match case").changed() {
                self.tabs[self.active].find_hit = None;
                self.find(true);
            }
            if self.find.not_found {
                ui.colored_label(ui.visuals().error_fg_color, "No results");
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let close = egui::Button::new("×").frame_when_inactive(false);
                if ui.add(close).on_hover_text("Close (Esc)").clicked() {
                    actions.push(Action::ToggleFind);
                }
            });
        });
        if ui.input(|i| i.key_pressed(Key::Escape)) {
            actions.push(Action::ToggleFind);
        }
    }

    fn editor(&mut self, ui: &mut egui::Ui, p: &Palette) {
        let dark = ui.visuals().dark_mode;
        let find_open = self.find.open;
        let tab = &mut self.tabs[self.active];
        let id = tab.editor_id();
        let font = FontId::monospace(EDITOR_FONT_SIZE);
        let (row_height, digit_width) =
            ui.fonts_mut(|f| (f.row_height(&font), f.glyph_width(&font, '0')));

        // The gutter is wide enough for the largest line number.
        let line_count = tab.text.bytes().filter(|&b| b == b'\n').count() + 1;
        let digits = line_count.to_string().len().max(3);
        let gutter_width = digits as f32 * digit_width + 22.0;
        let view = ui.available_rect_before_wrap(); // the visible editor area
        let margin = Margin::symmetric(8, 6);

        // A selection requested by open / find / snippets is applied to egui's
        // text-edit state before the widget runs, and scrolled into view after.
        let mut reveal_cursor = false;
        if let Some((a, b)) = tab.pending_select.take() {
            let mut state = egui::TextEdit::load_state(ui.ctx(), id).unwrap_or_default();
            let range = CCursorRange::two(CCursor::new(a), CCursor::new(b));
            state.cursor.set_char_range(Some(range));
            egui::TextEdit::store_state(ui.ctx(), id, state);
            ui.memory_mut(|m| m.request_focus(id));
            tab.sel = (a.min(b), a.max(b));
            reveal_cursor = true;
        }
        let reveal_hit = find_open && tab.find_hit.is_some() && ui.ctx().memory(|m| !m.has_focus(id));

        egui::ScrollArea::both()
            .id_salt(("editor_scroll", tab.id))
            .auto_shrink(false)
            .show(ui, |ui| {
                ui.horizontal_top(|ui| {
                    ui.spacing_mut().item_spacing.x = 0.0;
                    ui.add_space(gutter_width); // room for the line numbers

                    // Reserve paint slots *behind* the text for the current-line
                    // band and the search match; they are filled in below.
                    let band_slot = ui.painter().add(egui::Shape::Noop);
                    let hit_slot = ui.painter().add(egui::Shape::Noop);

                    // Highlighting is cached: re-tokenize only when the text or theme changes.
                    let cache = &mut tab.highlighted;
                    let mut layouter = |ui: &egui::Ui, text: &dyn egui::TextBuffer, _wrap_width: f32| {
                        let text = text.as_str();
                        let fresh = cache.as_ref().is_some_and(|h| h.dark == dark && h.text == text);
                        if !fresh {
                            *cache = Some(Highlighted {
                                text: text.to_owned(),
                                dark,
                                job: highlight(text, &font, row_height, p),
                            });
                        }
                        let job = cache.as_ref().expect("just filled").job.clone();
                        ui.fonts_mut(|f| f.layout_job(job))
                    };

                    let output = egui::TextEdit::multiline(&mut tab.text)
                        .id(id)
                        .font(font.clone())
                        .frame(egui::Frame::NONE)
                        .margin(margin)
                        .lock_focus(true) // Tab inserts a tab instead of moving focus
                        .desired_width(view.width() - gutter_width)
                        .min_size(egui::vec2(view.width() - gutter_width, view.height()))
                        .layouter(&mut layouter)
                        .show(ui);

                    if output.response.changed() {
                        tab.dirty = true;
                        tab.find_hit = None;
                    }

                    let galley = &output.galley;
                    let origin = output.galley_pos.to_vec2();
                    let painter = ui.painter();

                    // Remember cursor/selection while the editor has focus.
                    if let Some(range) = output.cursor_range {
                        let chars = range.as_sorted_char_range();
                        tab.sel = (chars.start.0, chars.end.0);
                        let at = range.primary.index.0;
                        if at != tab.line_col_at || output.response.changed() {
                            tab.line_col_at = at;
                            let cursor = byte_index(&tab.text, at);
                            let before = &tab.text[..cursor];
                            let line_start = before.rfind('\n').map_or(0, |i| i + 1);
                            tab.line_col = (
                                before.bytes().filter(|&b| b == b'\n').count() + 1,
                                before[line_start..].chars().count() + 1,
                            );
                        }
                        // Subtle band on the cursor line (only without a selection).
                        if range.is_empty() {
                            let row = galley.pos_from_cursor(range.primary).translate(origin);
                            let band = egui::Rect::from_x_y_ranges(
                                view.left()..=f32::INFINITY,
                                row.top()..=row.top() + row_height,
                            );
                            painter.set(band_slot, egui::Shape::rect_filled(band, 0.0, p.current_line));
                        }
                        if reveal_cursor {
                            let row = galley.pos_from_cursor(range.primary).translate(origin);
                            ui.scroll_to_rect(row.expand2(egui::vec2(60.0, 80.0)), None);
                        }
                    }

                    // Search match.
                    if let (true, Some((a, b))) = (find_open, tab.find_hit) {
                        let start = galley.pos_from_cursor(CCursor::new(a));
                        let end = galley.pos_from_cursor(CCursor::new(b));
                        let rect = egui::Rect::from_min_max(start.left_top(), end.right_bottom()).translate(origin);
                        painter.set(hit_slot, egui::Shape::rect_filled(rect.expand(1.0), 2.0, p.find_hit));
                        if reveal_hit {
                            ui.scroll_to_rect(rect.expand2(egui::vec2(60.0, 80.0)), None);
                        }
                    }

                    // Line numbers. They are painted at the left edge of the *view*,
                    // over the text, so they stay put when scrolling sideways.
                    let clip = ui.clip_rect();
                    let gutter = egui::Rect::from_min_max(
                        egui::pos2(view.left(), clip.top()),
                        egui::pos2(view.left() + gutter_width, clip.bottom()),
                    );
                    painter.rect_filled(gutter, 0.0, p.editor_bg);
                    let current_line = tab.line_col.0;
                    let mut line_number = 1;
                    let mut starts_line = true;
                    for row in &galley.rows {
                        let y = origin.y + row.pos.y;
                        if y > clip.bottom() {
                            break;
                        }
                        if starts_line && y + row_height >= clip.top() {
                            let color = if line_number == current_line { p.text } else { p.gutter_fg };
                            painter.text(
                                egui::pos2(gutter.right() - 12.0, y),
                                egui::Align2::RIGHT_TOP,
                                line_number,
                                font.clone(),
                                color,
                            );
                        }
                        starts_line = row.ends_with_newline;
                        if starts_line {
                            line_number += 1;
                        }
                    }

                    // Auto-indent: after Enter, repeat the indentation of the line above.
                    let cursor = output.cursor_range.and_then(|range| range.single());
                    let enter = output.response.changed() && ui.input(|i| i.key_pressed(Key::Enter));
                    if let (true, Some(cursor)) = (enter, cursor) {
                        let at = byte_index(&tab.text, cursor.index.0);
                        if tab.text[..at].ends_with('\n') {
                            let above = &tab.text[..at - 1];
                            let above = &above[above.rfind('\n').map_or(0, |i| i + 1)..];
                            let indent: String =
                                above.chars().take_while(|c| matches!(c, ' ' | '\t')).collect();
                            if !indent.is_empty() {
                                tab.text.insert_str(at, &indent);
                                let end = cursor.index.0 + indent.chars().count();
                                tab.pending_select = Some((end, end));
                                ui.ctx().request_repaint();
                            }
                        }
                    }
                });
            });
    }

    fn console_pane(&mut self, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
        apply_console_visuals(ui);
        let ctx = ui.ctx().clone();
        let font = FontId::monospace(CONSOLE_FONT_SIZE);
        let running = self.job.is_some();

        // Header strip
        egui::Panel::top("console_header")
            .frame(egui::Frame::new().fill(CONSOLE_BAR).inner_margin(Margin::symmetric(10, 3)))
            .show_separator_line(false)
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new("CONSOLE").small().strong().color(CONSOLE_DIM));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.small_button("Clear").on_hover_text(ctx.format_shortcut(&SC_CLEAR)).clicked() {
                            actions.push(Action::ClearConsole);
                        }
                        if ui.small_button("Copy all").clicked() {
                            actions.push(Action::CopyConsole);
                        }
                    });
                });
            });

        // Input line
        egui::Panel::bottom("console_input")
            .frame(egui::Frame::new().fill(CONSOLE_BAR).inner_margin(Margin::symmetric(10, 5)))
            .show_separator_line(false)
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 6.0;
                    let prompt = if running {
                        "stdin >".to_owned()
                    } else {
                        format!("{} $", pretty_path(&self.cwd))
                    };
                    ui.label(RichText::new(prompt).font(font.clone()).color(CONSOLE_PROMPT));

                    let hint = if running {
                        "input for the running script · Enter sends · Ctrl+D ends input · Ctrl+C stops"
                    } else {
                        "type a command and press Enter · ↑/↓ history"
                    };
                    let field = ui.add(
                        egui::TextEdit::singleline(&mut self.input)
                            .font(font.clone())
                            .frame(egui::Frame::NONE)
                            .hint_text(RichText::new(hint).color(CONSOLE_DIM))
                            .desired_width(f32::INFINITY),
                    );
                    if std::mem::take(&mut self.focus_input) {
                        field.request_focus();
                    }

                    if field.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                        self.submit_input(&ctx);
                        field.request_focus();
                    } else if field.has_focus() {
                        let (up, down, ctrl_d, ctrl_c) = ui.input(|i| {
                            (
                                i.key_pressed(Key::ArrowUp),
                                i.key_pressed(Key::ArrowDown),
                                i.modifiers.command && i.key_pressed(Key::D),
                                // egui turns Ctrl+C into a Copy event
                                i.events.iter().any(|e| matches!(e, egui::Event::Copy)),
                            )
                        });
                        if up || down {
                            self.browse_history(up);
                            // Put the cursor at the end of the recalled command.
                            let mut state = egui::TextEdit::load_state(&ctx, field.id).unwrap_or_default();
                            let end = CCursor::new(self.input.chars().count());
                            state.cursor.set_char_range(Some(CCursorRange::one(end)));
                            egui::TextEdit::store_state(&ctx, field.id, state);
                        }
                        if let Some(job) = &mut self.job {
                            if ctrl_d && job.stdin.take().is_some() {
                                self.console.line(Kind::Info, "[end of input]");
                            }
                            if ctrl_c && self.input.is_empty() {
                                actions.push(Action::Stop);
                            }
                        }
                    }
                });
            });

        // Output
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(CONSOLE_BG).inner_margin(Margin::symmetric(10, 6)))
            .show(ui, |ui| {
                let row_height = ui.fonts_mut(|f| f.row_height(&font));
                ui.spacing_mut().item_spacing.y = 0.0;
                egui::ScrollArea::both()
                    .id_salt("console_output")
                    .auto_shrink(false)
                    .stick_to_bottom(true)
                    // Only the visible rows are laid out, so 20 000 lines cost nothing.
                    .show_rows(ui, row_height, self.console.lines.len(), |ui, rows| {
                        for line in &self.console.lines[rows] {
                            let text = if line.text.is_empty() { " " } else { line.text.as_str() };
                            let text = RichText::new(text).font(font.clone()).color(line.kind.color());
                            ui.add(egui::Label::new(text).extend());
                        }
                    });
            });
    }

    fn browse_history(&mut self, older: bool) {
        if self.history.is_empty() {
            return;
        }
        let last = self.history.len() - 1;
        self.history_pos = match (self.history_pos, older) {
            (None, true) => Some(last),
            (None, false) => None,
            (Some(i), true) => Some(i.saturating_sub(1)),
            (Some(i), false) if i >= last => None,
            (Some(i), false) => Some(i + 1),
        };
        self.input = match self.history_pos {
            Some(i) => self.history[i].clone(),
            None => String::new(),
        };
    }

    fn status_bar(&mut self, ui: &mut egui::Ui, p: &Palette) {
        let tab = &self.tabs[self.active];
        ui.horizontal(|ui| {
            match (&self.job, &self.last_result) {
                (Some(job), _) => {
                    ui.spinner();
                    ui.label(format!("Running · {:.1}s", job.started.elapsed().as_secs_f32()));
                }
                (None, Some((ok, summary))) => {
                    let color = if *ok { p.run } else { p.stop };
                    ui.colored_label(color, "●");
                    ui.label(summary);
                }
                (None, None) => {
                    ui.label("Ready");
                }
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label("bash");
                ui.separator();
                let selected = tab.sel.1 - tab.sel.0;
                if selected > 0 {
                    ui.label(format!("{selected} selected"));
                    ui.separator();
                }
                ui.label(format!("Ln {}, Col {}", tab.line_col.0, tab.line_col.1));
                ui.separator();
                ui.label(pretty_path(&self.cwd)).on_hover_text("Working directory (change it with cd in the console)");
            });
        });
    }

    fn confirm_dialog(&mut self, ctx: &egui::Context) {
        let Some(confirm) = self.confirm else { return };
        let message = match confirm {
            Confirm::CloseTab(i) => match self.tabs.get(i) {
                Some(tab) => format!("Save changes to {}?", tab.name),
                None => {
                    self.confirm = None;
                    return;
                }
            },
            Confirm::Quit => {
                let n = self.tabs.iter().filter(|t| t.dirty).count();
                format!("{n} script{} with unsaved changes. Save before quitting?", if n == 1 { "" } else { "s" })
            }
        };

        // None = still open, Some(None) = cancel, Some(Some(save)) = decided
        let mut answer: Option<Option<bool>> = None;
        let modal = egui::Modal::new(egui::Id::unique("bash-ise unsaved changes")).show(ctx, |ui| {
            ui.set_width(380.0);
            ui.heading("Unsaved changes");
            ui.add_space(4.0);
            ui.label(message);
            ui.add_space(12.0);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("Cancel").clicked() {
                    answer = Some(None);
                }
                if ui.button("Don't save").clicked() {
                    answer = Some(Some(false));
                }
                if ui.button(RichText::new("Save").strong()).clicked() {
                    answer = Some(Some(true));
                }
            });
        });
        if modal.should_close() && answer.is_none() {
            answer = Some(None); // Esc or click outside
        }
        if let Some(answer) = answer {
            self.confirm = None;
            if let Some(save) = answer {
                self.resolve_confirm(confirm, save, ctx);
            }
        }
    }
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().into_owned())
}

fn menu_item(
    ui: &mut egui::Ui,
    label: &str,
    shortcut: Option<KeyboardShortcut>,
    enabled: bool,
    action: Action,
    actions: &mut Vec<Action>,
) {
    let mut button = egui::Button::new(label);
    if let Some(shortcut) = shortcut {
        button = button.shortcut_text(ui.ctx().format_shortcut(&shortcut));
    }
    if ui.add_enabled(enabled, button).clicked() {
        actions.push(action);
    }
}

fn tool_button(
    ui: &mut egui::Ui,
    label: impl Into<egui::WidgetText>,
    tooltip: &str,
    shortcut: KeyboardShortcut,
    enabled: bool,
    action: Action,
    actions: &mut Vec<Action>,
) {
    let tooltip = format!("{tooltip}  ({})", ui.ctx().format_shortcut(&shortcut));
    let button = egui::Button::new(label.into()).frame_when_inactive(false);
    if ui.add_enabled(enabled, button).on_hover_text(tooltip).clicked() {
        actions.push(action);
    }
}

impl eframe::App for BashIse {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        let p = Palette::of(ui.visuals().dark_mode);
        let mut actions: Vec<Action> = Vec::new();

        // 1. Background work and window-level events
        self.poll_job(&ctx);

        let (close_requested, dropped) =
            ctx.input(|i| (i.viewport().close_requested(), i.raw.dropped_files.clone()));
        if close_requested && !self.allow_close && self.tabs.iter().any(|t| t.dirty) {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.confirm = Some(Confirm::Quit);
        }
        for file in dropped {
            self.open_path(file.path()); // drag & drop from the file manager
        }
        if self.confirm.is_none() {
            self.shortcuts(&ctx, &mut actions);
        }

        // 2. Panels, from the outside in. Whatever is left is the editor.
        let bar = |fill: Color32, x: i8, y: i8| egui::Frame::new().fill(fill).inner_margin(Margin::symmetric(x, y));

        egui::Panel::top("menu")
            .frame(bar(p.chrome, 8, 3))
            .show_separator_line(false)
            .show(ui, |ui| self.menu_bar(ui, &mut actions));
        egui::Panel::top("toolbar")
            .frame(bar(p.chrome, 8, 5))
            .show(ui, |ui| self.toolbar(ui, &p, &mut actions));
        egui::Panel::bottom("status")
            .frame(bar(p.chrome, 10, 3))
            .show(ui, |ui| self.status_bar(ui, &p));
        if self.show_console {
            egui::Panel::bottom("console")
                .frame(egui::Frame::new().fill(CONSOLE_BG))
                .resizable(true)
                .default_size(250.0)
                .min_size(110.0)
                .show(ui, |ui| self.console_pane(ui, &mut actions));
        }
        egui::Panel::top("tabs")
            .frame(egui::Frame::new().fill(p.tab_bar).inner_margin(Margin { left: 6, right: 6, top: 5, bottom: 0 }))
            .show_separator_line(false)
            .show(ui, |ui| self.tab_bar(ui, &p, &mut actions));
        if self.find.open {
            egui::Panel::top("find")
                .frame(bar(p.chrome, 10, 5))
                .show(ui, |ui| self.find_bar(ui, &mut actions));
        }
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(p.editor_bg))
            .show(ui, |ui| self.editor(ui, &p));

        // 3. Dialogs, then apply what the user asked for
        self.confirm_dialog(&ctx);
        for action in actions {
            self.apply(action, &ctx);
        }

        // 4. Window title follows the active tab
        let tab = &self.tabs[self.active];
        let title = format!("{}{} — Bash ISE", if tab.dirty { "● " } else { "" }, tab.name);
        if title != self.window_title {
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(title.clone()));
            self.window_title = title;
        }
    }

    fn on_exit(&mut self) {
        // Do not leave a running script behind when the window closes.
        if let Some(job) = &self.job {
            signal_group(job.pid, libc::SIGTERM);
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests for the parts that do not need a window: `cargo test`
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// (text, token) pairs; plain pieces are trimmed and dropped if only whitespace.
    fn toks(src: &str) -> Vec<(&str, Tok)> {
        let tokens = tokenize(src);
        // The pieces must cover the source exactly, in order.
        let mut at = 0;
        for (range, _) in &tokens {
            assert_eq!(range.start, at, "gap or overlap in {src:?}");
            at = range.end;
        }
        assert_eq!(at, src.len(), "tokens do not reach the end of {src:?}");
        tokens
            .into_iter()
            .map(|(range, tok)| match tok {
                Tok::Plain => (src[range].trim(), tok),
                _ => (&src[range], tok),
            })
            .filter(|(text, _)| !text.is_empty())
            .collect()
    }

    #[test]
    fn commands_arguments_and_variables() {
        use Tok::*;
        assert_eq!(
            toks("echo -n \"hi $USER\" 42 # done"),
            [("echo", Command), ("-n", Param), ("\"hi ", Str), ("$USER", Var), ("\"", Str), ("42", Number), ("# done", Comment)]
        );
        assert_eq!(
            toks("name=World; export PATH"),
            [("name", Var), ("=", Operator), ("World", Plain), (";", Operator), ("export", Keyword), ("PATH", Var)]
        );
    }

    #[test]
    fn keywords_and_pipelines() {
        use Tok::*;
        assert_eq!(
            toks("if ls | grep -q x; then echo in; fi"),
            [
                ("if", Keyword), ("ls", Command), ("|", Operator), ("grep", Command), ("-q", Param), ("x", Plain),
                (";", Operator), ("then", Keyword), ("echo", Command), ("in", Keyword), (";", Operator), ("fi", Keyword)
            ]
        );
        assert_eq!(
            toks("for f in *.txt; do wc -l \"$f\"; done"),
            [
                ("for", Keyword), ("f", Var), ("in", Keyword), ("*.txt", Plain), (";", Operator), ("do", Keyword),
                ("wc", Command), ("-l", Param), ("\"", Str), ("$f", Var), ("\"", Str), (";", Operator), ("done", Keyword)
            ]
        );
    }

    #[test]
    fn command_substitution_inside_a_string() {
        use Tok::*;
        assert_eq!(
            toks("x=\"$(basename \"$f\") ok\""),
            [
                ("x", Var), ("=", Operator), ("\"", Str), ("$(", Operator), ("basename", Command), ("\"", Str),
                ("$f", Var), ("\"", Str), (")", Operator), (" ok\"", Str)
            ]
        );
    }

    #[test]
    fn heredoc_body_is_one_string() {
        use Tok::*;
        let src = "cat <<EOF\ndon't $panic\nEOF\necho done\n";
        assert_eq!(
            toks(src),
            [("cat", Command), ("<<", Operator), ("EOF", Str), ("don't $panic\nEOF\n", Str), ("echo", Command), ("done", Plain)]
        );
    }

    #[test]
    fn case_patterns_are_not_commands() {
        use Tok::*;
        let src = "case $1 in\n  start|stop) run ;;\n  *) exit 1 ;;\nesac";
        assert_eq!(
            toks(src),
            [
                ("case", Keyword), ("$1", Var), ("in", Keyword), ("start", Plain), ("|", Operator), ("stop", Plain),
                (")", Operator), ("run", Command), (";;", Operator), ("*", Plain), (")", Operator), ("exit", Keyword),
                ("1", Number), (";;", Operator), ("esac", Keyword)
            ]
        );
    }

    #[test]
    fn tokenizer_survives_odd_input() {
        for src in ["", "\"unterminated", "'x", "$", "${", "echo \\", "é=\"ü\\é\" # ñ", "a <<", "$((1 +", "x=$(", ")))"] {
            toks(src);
        }
    }

    #[test]
    fn console_handles_partial_lines_and_carriage_returns() {
        let mut c = Console::default();
        c.push(Kind::Out, "Name: ");
        c.push(Kind::Out, "Bob\nnext");
        c.push(Kind::Err, "oops\n");
        c.push(Kind::Out, "10%\r");
        c.push(Kind::Out, "50%\r100%\ndone\r\n");
        let lines: Vec<&str> = c.lines.iter().map(|l| l.text.as_str()).collect();
        assert_eq!(lines, ["Name: Bob", "next", "oops", "100%", "done"]);
    }

    #[test]
    fn ansi_colours_are_removed() {
        assert_eq!(strip_ansi("\x1b[1;31mred\x1b[0m plain \x1b]0;title\x07!"), "red plain !");
    }

    #[test]
    fn utf8_split_across_chunks() {
        let bytes = "é".as_bytes();
        let mut pending = vec![b'a', bytes[0]];
        assert_eq!(take_utf8(&mut pending), "a");
        pending.push(bytes[1]);
        assert_eq!(take_utf8(&mut pending), "é");
        assert!(pending.is_empty());
    }

    #[test]
    fn selection_or_current_line() {
        let mut tab = Tab::new(1, "t".into(), None, "one\ntwo é\nthree".into());
        tab.sel = (5, 5);
        assert_eq!(tab.selection_or_line(), "two é");
        tab.sel = (4, 9);
        assert_eq!(tab.selection_or_line(), "two é");
        tab.sel = (0, 0);
        assert_eq!(tab.selection_or_line(), "one");
    }
}
