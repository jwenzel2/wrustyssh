use slint::{Image, Rgb8Pixel, SharedPixelBuffer};
use std::collections::HashMap;
use std::path::PathBuf;

/// Terminal renderer: takes vt100 screen state and renders to a pixel buffer.
pub struct TerminalRenderer {
    pub parser: vt100::Parser,
    font: fontdue::Font,
    palette: TerminalPalette,
    cell_width: usize,
    cell_height: usize,
    baseline: usize,
    glyph_cache: HashMap<(char, bool, u32), (fontdue::Metrics, Vec<u8>)>,
    /// Tracks whether we are inside a CSI escape sequence so we can patch
    /// unhandled final bytes (e.g. HVP 'f' -> CUP 'H') before the vt100 parser sees them.
    csi_state: CsiScanState,
    scrollback_limit: usize,
    available_scrollback: usize,
    viewport_scrollback: usize,
    selection: Option<TerminalSelection>,
    last_size: (u16, u16),
}

/// Minimal state machine for detecting CSI sequences in a byte stream.
/// Needed because sequences can be split across data chunks.
#[derive(Clone, Copy, PartialEq)]
enum CsiScanState {
    Normal,
    /// Saw ESC (0x1b), waiting for '['
    Esc,
    /// Inside a CSI sequence (parameter / intermediate bytes)
    CsiParam,
}

static TERMINAL_FONT_FALLBACK: &[u8] = include_bytes!("../../fonts/CascadiaCode.ttf");

/// Snapshot of a single cell for rendering (avoids borrow conflicts).
struct CellSnapshot {
    row: u16,
    col: u16,
    ch: char,
    bold: bool,
    fg: vt100::Color,
    bg: vt100::Color,
    is_cursor: bool,
    is_selected: bool,
}

#[derive(Clone, Copy)]
struct TerminalSelection {
    start: CellPosition,
    end: CellPosition,
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct CellPosition {
    row: u16,
    col: u16,
}

#[derive(Clone, Copy)]
struct TerminalPalette {
    default_bg: (u8, u8, u8),
    default_fg: (u8, u8, u8),
    ansi: [(u8, u8, u8); 16],
}

impl TerminalRenderer {
    pub fn new(
        cols: u16,
        rows: u16,
        font_family: &str,
        font_size: f32,
        scrollback_len: usize,
        color_scheme: &str,
    ) -> Self {
        let font = load_terminal_font(font_family);
        let palette = terminal_palette_by_name(color_scheme);

        let metrics = font.metrics('M', font_size);
        let cell_width = metrics
            .advance_width
            .max(metrics.width as f32 + metrics.xmin.max(0) as f32)
            .ceil() as usize
            + 1; // extra pixel for letter spacing
        let line_metrics = font.horizontal_line_metrics(font_size).unwrap();
        let ascent = line_metrics.ascent.ceil() as usize;
        let cell_height = (line_metrics.ascent - line_metrics.descent + line_metrics.line_gap)
            .max(metrics.height as f32 + metrics.ymin.unsigned_abs() as f32)
            .ceil() as usize
            + 1;

        let parser = vt100::Parser::new(rows, cols, scrollback_len);

        Self {
            parser,
            font,
            palette,
            cell_width: cell_width.max(1),
            cell_height: cell_height.max(1),
            baseline: ascent,
            glyph_cache: HashMap::new(),
            csi_state: CsiScanState::Normal,
            scrollback_limit: scrollback_len,
            available_scrollback: 0,
            viewport_scrollback: 0,
            selection: None,
            last_size: (rows, cols),
        }
    }

    #[allow(dead_code)]
    pub fn cell_size(&self) -> (usize, usize) {
        (self.cell_width, self.cell_height)
    }

    pub fn process(&mut self, data: &[u8]) {
        // The vt100 crate (0.15) does not handle CSI f (HVP – Horizontal and
        // Vertical Position).  Programs like apt use HVP to position the cursor
        // at the progress-bar row.  HVP is functionally identical to CUP
        // (CSI H), so we rewrite the final byte before the parser sees it.
        let mut patched = Vec::new(); // allocated lazily
        for (i, &b) in data.iter().enumerate() {
            match self.csi_state {
                CsiScanState::Normal => {
                    if b == 0x1b {
                        self.csi_state = CsiScanState::Esc;
                    }
                }
                CsiScanState::Esc => {
                    if b == b'[' {
                        self.csi_state = CsiScanState::CsiParam;
                    } else {
                        self.csi_state = CsiScanState::Normal;
                    }
                }
                CsiScanState::CsiParam => {
                    // Parameter bytes: 0x30–0x3F  Intermediate bytes: 0x20–0x2F
                    // Final byte: 0x40–0x7E
                    if (0x20..=0x3F).contains(&b) {
                        // still in parameters / intermediates
                    } else if (0x40..=0x7E).contains(&b) {
                        // final byte – patch 'f' (HVP) to 'H' (CUP)
                        if b == b'f' {
                            if patched.is_empty() {
                                patched = data.to_vec();
                            }
                            patched[i] = b'H';
                        }
                        self.csi_state = CsiScanState::Normal;
                    } else {
                        // unexpected byte – abort CSI parse
                        self.csi_state = CsiScanState::Normal;
                    }
                }
            }
        }
        if patched.is_empty() {
            self.parser.process(data);
        } else {
            self.parser.process(&patched);
        }
        self.available_scrollback = self
            .available_scrollback
            .saturating_add(count_scrollback_lines(data))
            .min(self.scrollback_limit);
        if self.viewport_scrollback == 0 {
            self.parser.set_scrollback(0);
        } else {
            let max_scrollback = self.available_scrollback;
            self.viewport_scrollback = self.viewport_scrollback.min(max_scrollback);
            self.parser.set_scrollback(self.viewport_scrollback);
        }
    }

    #[allow(dead_code)]
    pub fn set_size(&mut self, rows: u16, cols: u16) {
        self.parser.set_size(rows, cols);
        self.last_size = (rows, cols);
        self.viewport_scrollback = self.viewport_scrollback.min(self.available_scrollback);
        self.parser.set_scrollback(self.viewport_scrollback);
        self.clamp_selection();
    }

    pub fn scroll_viewport(&mut self, delta_rows: i32) {
        let max_scrollback = self.available_scrollback;
        let next = (self.viewport_scrollback as i32 + delta_rows).clamp(0, max_scrollback as i32);
        self.viewport_scrollback = next as usize;
        self.parser.set_scrollback(self.viewport_scrollback);
        self.clamp_selection();
    }

    pub fn reset_viewport_to_bottom(&mut self) {
        self.viewport_scrollback = 0;
        self.parser.set_scrollback(0);
    }

    pub fn clear_selection(&mut self) {
        self.selection = None;
    }

    pub fn begin_selection(&mut self, x: f32, y: f32) {
        if let Some(cell) = self.point_to_cell(x, y) {
            self.selection = Some(TerminalSelection {
                start: cell,
                end: cell,
            });
        } else {
            self.selection = None;
        }
    }

    pub fn update_selection(&mut self, x: f32, y: f32) {
        let Some(cell) = self.point_to_cell(x, y) else {
            return;
        };
        if let Some(selection) = self.selection.as_mut() {
            selection.end = cell;
        }
    }

    pub fn selected_text(&self) -> Option<String> {
        let selection = self.selection?;
        let (start, end) = ordered_positions(selection.start, selection.end);
        let text = self.parser.screen().contents_between(
            start.row,
            start.col,
            end.row,
            end.col.saturating_add(1),
        );
        if text.is_empty() {
            None
        } else {
            Some(text)
        }
    }

    pub fn render_to_size(
        &mut self,
        font_size: f32,
        target_width: usize,
        target_height: usize,
    ) -> Image {
        // Phase 1: Snapshot all cell data from the screen (immutable borrow of parser)
        let cells: Vec<CellSnapshot>;
        let rows: u16;
        let cols: u16;
        {
            let screen = self.parser.screen();
            let size = screen.size();
            rows = size.0;
            cols = size.1;
            let cursor = screen.cursor_position();

            let mut cell_list = Vec::with_capacity(rows as usize * cols as usize);
            for row in 0..rows {
                for col in 0..cols {
                    if let Some(cell) = screen.cell(row, col) {
                        let ch = cell.contents().chars().next().unwrap_or(' ');
                        cell_list.push(CellSnapshot {
                            row,
                            col,
                            ch,
                            bold: cell.bold(),
                            fg: cell.fgcolor(),
                            bg: cell.bgcolor(),
                            is_cursor: row == cursor.0 && col == cursor.1,
                            is_selected: self.is_cell_selected(row, col),
                        });
                    }
                }
            }
            cells = cell_list;
        }

        let grid_width = cols as usize * self.cell_width;
        let grid_height = rows as usize * self.cell_height;

        // Use target dimensions if provided, otherwise use grid dimensions
        let width = if target_width > 0 {
            target_width
        } else {
            grid_width
        };
        let height = if target_height > 0 {
            target_height
        } else {
            grid_height
        };

        if width == 0 || height == 0 {
            let buf = SharedPixelBuffer::<Rgb8Pixel>::new(1, 1);
            return Image::from_rgb8(buf);
        }

        let mut buf = SharedPixelBuffer::<Rgb8Pixel>::new(width as u32, height as u32);
        let pixels = buf.make_mut_bytes();

        // Fill the terminal with the Campbell-style near-black background.
        for chunk in pixels.chunks_exact_mut(3) {
            chunk[0] = self.palette.default_bg.0;
            chunk[1] = self.palette.default_bg.1;
            chunk[2] = self.palette.default_bg.2;
        }

        // Phase 2: Render each cell (can now mutably borrow self for glyph cache)
        for cs in &cells {
            let (bg_r, bg_g, bg_b) = self.vt100_color_to_rgb(cs.bg, false);
            let (fg_r, fg_g, fg_b) = self.vt100_color_to_rgb(cs.fg, true);

            let (draw_bg_r, draw_bg_g, draw_bg_b, draw_fg_r, draw_fg_g, draw_fg_b) =
                if cs.is_selected {
                    (38, 79, 120, 240, 246, 255)
                } else if cs.is_cursor {
                    (fg_r, fg_g, fg_b, bg_r, bg_g, bg_b)
                } else {
                    (bg_r, bg_g, bg_b, fg_r, fg_g, fg_b)
                };

            let x0 = cs.col as usize * self.cell_width;
            let y0 = cs.row as usize * self.cell_height;

            // Draw background
            for dy in 0..self.cell_height {
                let py = y0 + dy;
                if py >= height {
                    break;
                }
                for dx in 0..self.cell_width {
                    let px = x0 + dx;
                    if px >= width {
                        break;
                    }
                    let offset = (py * width + px) * 3;
                    if offset + 2 < pixels.len() {
                        pixels[offset] = draw_bg_r;
                        pixels[offset + 1] = draw_bg_g;
                        pixels[offset + 2] = draw_bg_b;
                    }
                }
            }

            // Draw glyph
            if cs.ch != ' ' && cs.ch != '\0' {
                let (gm, glyph_data) = self.rasterize_glyph(cs.ch, cs.bold, font_size);

                let glyph_y_offset =
                    (self.baseline as i32 - gm.height as i32 - gm.ymin as i32).max(0) as usize;
                let glyph_x_offset = gm.xmin.max(0) as usize;

                for gy in 0..gm.height {
                    let py = y0 + glyph_y_offset + gy;
                    if py >= height {
                        break;
                    }
                    for gx in 0..gm.width {
                        let px = x0 + glyph_x_offset + gx;
                        if px >= width {
                            break;
                        }
                        let raw_alpha = glyph_data[gy * gm.width + gx];
                        if raw_alpha > 0 {
                            let offset = (py * width + px) * 3;
                            if offset + 2 < pixels.len() {
                                // Gamma-boost alpha to match Windows Terminal's bolder text rendering
                                let normalized = raw_alpha as f32 / 255.0;
                                let boosted = normalized.powf(0.2);
                                let a = (boosted * 255.0) as u16;
                                let inv_a = 255 - a;
                                pixels[offset] = ((draw_fg_r as u16 * a
                                    + pixels[offset] as u16 * inv_a)
                                    / 255) as u8;
                                pixels[offset + 1] =
                                    ((draw_fg_g as u16 * a + pixels[offset + 1] as u16 * inv_a)
                                        / 255) as u8;
                                pixels[offset + 2] =
                                    ((draw_fg_b as u16 * a + pixels[offset + 2] as u16 * inv_a)
                                        / 255) as u8;
                            }
                        }
                    }
                }
            }
        }

        Image::from_rgb8(buf)
    }

    fn point_to_cell(&self, x: f32, y: f32) -> Option<CellPosition> {
        if x.is_sign_negative() || y.is_sign_negative() {
            return None;
        }
        let row = (y as usize / self.cell_height) as u16;
        let col = (x as usize / self.cell_width) as u16;
        let (rows, cols) = self.last_size;
        Some(CellPosition {
            row: row.min(rows.saturating_sub(1)),
            col: col.min(cols.saturating_sub(1)),
        })
    }

    fn is_cell_selected(&self, row: u16, col: u16) -> bool {
        let Some(selection) = self.selection else {
            return false;
        };
        let (start, end) = ordered_positions(selection.start, selection.end);
        let current = CellPosition { row, col };
        current >= start && current <= end
    }

    fn clamp_selection(&mut self) {
        let Some(selection) = self.selection else {
            return;
        };
        let (rows, cols) = self.last_size;
        if rows == 0 || cols == 0 {
            self.selection = None;
            return;
        }

        self.selection = Some(TerminalSelection {
            start: CellPosition {
                row: selection.start.row.min(rows - 1),
                col: selection.start.col.min(cols - 1),
            },
            end: CellPosition {
                row: selection.end.row.min(rows - 1),
                col: selection.end.col.min(cols - 1),
            },
        });
    }

    fn rasterize_glyph(
        &mut self,
        ch: char,
        bold: bool,
        font_size: f32,
    ) -> (fontdue::Metrics, Vec<u8>) {
        let cache_key = (ch, bold, font_size.to_bits());
        if let Some(cached) = self.glyph_cache.get(&cache_key) {
            return cached.clone();
        }

        let (metrics, bitmap) = self.font.rasterize(ch, font_size);
        self.glyph_cache
            .insert(cache_key, (metrics, bitmap.clone()));
        (metrics, bitmap)
    }

    fn vt100_color_to_rgb(&self, color: vt100::Color, is_fg: bool) -> (u8, u8, u8) {
        match color {
            vt100::Color::Default => {
                if is_fg {
                    self.palette.default_fg
                } else {
                    self.palette.default_bg
                }
            }
            vt100::Color::Idx(idx) => ansi_256_to_rgb(idx, &self.palette),
            vt100::Color::Rgb(r, g, b) => (r, g, b),
        }
    }
}

/// Convert key event text from Slint into VT escape sequences.
pub fn translate_key(
    text: &str,
    ctrl: bool,
    _shift: bool,
    alt: bool,
    _meta: bool,
) -> Option<Vec<u8>> {
    if ctrl && text.len() == 1 {
        let ch = text.chars().next().unwrap();
        if ch.is_ascii_alphabetic() {
            let ctrl_code = (ch.to_ascii_uppercase() as u8) - b'@';
            return Some(vec![ctrl_code]);
        }
    }

    if alt && text.len() == 1 {
        let ch = text.as_bytes()[0];
        return Some(vec![0x1b, ch]);
    }

    match text {
        "\u{f700}" | "Up" => Some(b"\x1b[A".to_vec()),
        "\u{f701}" | "Down" => Some(b"\x1b[B".to_vec()),
        "\u{f703}" | "Right" => Some(b"\x1b[C".to_vec()),
        "\u{f702}" | "Left" => Some(b"\x1b[D".to_vec()),
        "\u{f729}" | "Home" => Some(b"\x1b[H".to_vec()),
        "\u{f72b}" | "End" => Some(b"\x1b[F".to_vec()),
        "\u{f72c}" | "PageUp" => Some(b"\x1b[5~".to_vec()),
        "\u{f72d}" | "PageDown" => Some(b"\x1b[6~".to_vec()),
        "\u{f728}" | "Delete" => Some(b"\x1b[3~".to_vec()),
        "\u{f727}" | "Insert" => Some(b"\x1b[2~".to_vec()),
        "\u{f704}" | "F1" => Some(b"\x1bOP".to_vec()),
        "\u{f705}" | "F2" => Some(b"\x1bOQ".to_vec()),
        "\u{f706}" | "F3" => Some(b"\x1bOR".to_vec()),
        "\u{f707}" | "F4" => Some(b"\x1bOS".to_vec()),
        "\u{f708}" | "F5" => Some(b"\x1b[15~".to_vec()),
        "\u{f709}" | "F6" => Some(b"\x1b[17~".to_vec()),
        "\u{f70a}" | "F7" => Some(b"\x1b[18~".to_vec()),
        "\u{f70b}" | "F8" => Some(b"\x1b[19~".to_vec()),
        "\u{f70c}" | "F9" => Some(b"\x1b[20~".to_vec()),
        "\u{f70d}" | "F10" => Some(b"\x1b[21~".to_vec()),
        "\u{f70e}" | "F11" => Some(b"\x1b[23~".to_vec()),
        "\u{f70f}" | "F12" => Some(b"\x1b[24~".to_vec()),
        "\n" | "\r" => Some(b"\r".to_vec()),
        "\t" => Some(b"\t".to_vec()),
        "\u{8}" | "\u{7f}" => Some(b"\x7f".to_vec()),
        "\u{1b}" => Some(b"\x1b".to_vec()),
        _ => {
            if !text.is_empty() && !text.starts_with('\u{f7}') && !text.starts_with('\u{f6}') {
                Some(text.as_bytes().to_vec())
            } else {
                None
            }
        }
    }
}

fn ansi_256_to_rgb(idx: u8, palette: &TerminalPalette) -> (u8, u8, u8) {
    match idx {
        0..=15 => palette.ansi[idx as usize],
        16..=231 => {
            let idx = idx - 16;
            let b = (idx % 6) * 51;
            let g = ((idx / 6) % 6) * 51;
            let r = (idx / 36) * 51;
            (r, g, b)
        }
        232..=255 => {
            let v = 8 + (idx - 232) * 10;
            (v, v, v)
        }
    }
}

pub fn terminal_palette_names() -> Vec<&'static str> {
    vec![
        "Campbell",
        "Campbell Powershell",
        "One Half Dark",
        "One Half Light",
        "Tango Dark",
        "Tango Light",
        "Solarized Dark",
        "Solarized Light",
        "Vintage",
    ]
}

pub fn terminal_palette_index(name: &str) -> i32 {
    terminal_palette_names()
        .iter()
        .position(|candidate| candidate.eq_ignore_ascii_case(name))
        .unwrap_or(0) as i32
}

pub fn terminal_palette_name_by_index(index: i32) -> &'static str {
    terminal_palette_names()
        .get(index.max(0) as usize)
        .copied()
        .unwrap_or("Campbell")
}

fn terminal_palette_by_name(name: &str) -> TerminalPalette {
    match name.to_ascii_lowercase().as_str() {
        "campbell powershell" => TerminalPalette {
            default_bg: (1, 36, 86),
            default_fg: (242, 242, 242),
            ansi: [
                (12, 12, 12),
                (197, 15, 31),
                (19, 161, 14),
                (193, 156, 0),
                (0, 55, 218),
                (136, 23, 152),
                (58, 150, 221),
                (204, 204, 204),
                (118, 118, 118),
                (231, 72, 86),
                (22, 198, 12),
                (249, 241, 165),
                (59, 120, 255),
                (180, 0, 158),
                (97, 214, 214),
                (242, 242, 242),
            ],
        },
        "one half dark" => TerminalPalette {
            default_bg: (40, 44, 52),
            default_fg: (220, 223, 228),
            ansi: [
                (40, 44, 52),
                (229, 192, 123),
                (152, 195, 121),
                (229, 192, 123),
                (97, 175, 239),
                (198, 120, 221),
                (86, 182, 194),
                (220, 223, 228),
                (92, 99, 112),
                (224, 108, 117),
                (152, 195, 121),
                (229, 192, 123),
                (97, 175, 239),
                (198, 120, 221),
                (86, 182, 194),
                (255, 255, 255),
            ],
        },
        "one half light" => TerminalPalette {
            default_bg: (250, 250, 250),
            default_fg: (56, 58, 66),
            ansi: [
                (56, 58, 66),
                (228, 86, 73),
                (80, 161, 79),
                (193, 132, 1),
                (1, 119, 210),
                (166, 38, 164),
                (9, 151, 179),
                (250, 250, 250),
                (160, 161, 167),
                (228, 86, 73),
                (80, 161, 79),
                (193, 132, 1),
                (1, 119, 210),
                (166, 38, 164),
                (9, 151, 179),
                (255, 255, 255),
            ],
        },
        "tango dark" => TerminalPalette {
            default_bg: (46, 52, 54),
            default_fg: (211, 215, 207),
            ansi: [
                (46, 52, 54),
                (204, 0, 0),
                (78, 154, 6),
                (196, 160, 0),
                (52, 101, 164),
                (117, 80, 123),
                (6, 152, 154),
                (211, 215, 207),
                (85, 87, 83),
                (239, 41, 41),
                (138, 226, 52),
                (252, 233, 79),
                (114, 159, 207),
                (173, 127, 168),
                (52, 226, 226),
                (238, 238, 236),
            ],
        },
        "tango light" => TerminalPalette {
            default_bg: (238, 238, 236),
            default_fg: (46, 52, 54),
            ansi: [
                (46, 52, 54),
                (204, 0, 0),
                (78, 154, 6),
                (196, 160, 0),
                (52, 101, 164),
                (117, 80, 123),
                (6, 152, 154),
                (211, 215, 207),
                (85, 87, 83),
                (239, 41, 41),
                (138, 226, 52),
                (252, 233, 79),
                (114, 159, 207),
                (173, 127, 168),
                (52, 226, 226),
                (255, 255, 255),
            ],
        },
        "solarized dark" => TerminalPalette {
            default_bg: (0, 43, 54),
            default_fg: (131, 148, 150),
            ansi: [
                (7, 54, 66),
                (220, 50, 47),
                (133, 153, 0),
                (181, 137, 0),
                (38, 139, 210),
                (211, 54, 130),
                (42, 161, 152),
                (238, 232, 213),
                (0, 43, 54),
                (203, 75, 22),
                (88, 110, 117),
                (101, 123, 131),
                (131, 148, 150),
                (108, 113, 196),
                (147, 161, 161),
                (253, 246, 227),
            ],
        },
        "solarized light" => TerminalPalette {
            default_bg: (253, 246, 227),
            default_fg: (101, 123, 131),
            ansi: [
                (7, 54, 66),
                (220, 50, 47),
                (133, 153, 0),
                (181, 137, 0),
                (38, 139, 210),
                (211, 54, 130),
                (42, 161, 152),
                (238, 232, 213),
                (0, 43, 54),
                (203, 75, 22),
                (88, 110, 117),
                (101, 123, 131),
                (131, 148, 150),
                (108, 113, 196),
                (147, 161, 161),
                (253, 246, 227),
            ],
        },
        "vintage" => TerminalPalette {
            default_bg: (47, 40, 31),
            default_fg: (201, 196, 184),
            ansi: [
                (47, 40, 31),
                (194, 54, 33),
                (37, 188, 36),
                (173, 173, 39),
                (73, 46, 225),
                (211, 56, 211),
                (51, 187, 200),
                (203, 204, 205),
                (129, 131, 131),
                (252, 57, 31),
                (49, 231, 34),
                (234, 236, 35),
                (88, 51, 255),
                (249, 53, 248),
                (20, 240, 240),
                (233, 235, 235),
            ],
        },
        _ => TerminalPalette {
            default_bg: (12, 12, 12),
            default_fg: (230, 230, 230),
            ansi: [
                (12, 12, 12),
                (197, 15, 31),
                (19, 161, 14),
                (193, 156, 0),
                (0, 55, 218),
                (136, 23, 152),
                (58, 150, 221),
                (204, 204, 204),
                (118, 118, 118),
                (231, 72, 86),
                (22, 198, 12),
                (249, 241, 165),
                (59, 120, 255),
                (180, 0, 158),
                (97, 214, 214),
                (242, 242, 242),
            ],
        },
    }
}

fn load_terminal_font(font_family: &str) -> fontdue::Font {
    for path in terminal_font_candidates(font_family) {
        if let Ok(bytes) = std::fs::read(&path) {
            if let Ok(font) = fontdue::Font::from_bytes(bytes, fontdue::FontSettings::default()) {
                return font;
            }
        }
    }

    fontdue::Font::from_bytes(TERMINAL_FONT_FALLBACK, fontdue::FontSettings::default())
        .expect("Failed to load fallback terminal font")
}

fn ordered_positions(a: CellPosition, b: CellPosition) -> (CellPosition, CellPosition) {
    if a <= b {
        (a, b)
    } else {
        (b, a)
    }
}

fn count_scrollback_lines(data: &[u8]) -> usize {
    data.iter().filter(|&&byte| byte == b'\n').count()
}

fn terminal_font_candidates(font_family: &str) -> Vec<PathBuf> {
    let windows_fonts = PathBuf::from(r"C:\Windows\Fonts");
    match font_family.to_ascii_lowercase().as_str() {
        "cascadia mono" => vec![
            windows_fonts.join("CascadiaMono.ttf"),
            windows_fonts.join("CascadiaCode.ttf"),
        ],
        "cascadia code" => vec![
            windows_fonts.join("CascadiaCode.ttf"),
            windows_fonts.join("CascadiaMono.ttf"),
        ],
        _ => vec![
            windows_fonts.join("CascadiaMono.ttf"),
            windows_fonts.join("CascadiaCode.ttf"),
        ],
    }
}
