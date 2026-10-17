//! Drawings shared by the screens: the step lane, the animated agent lanes and the four
//! explainer pictures. Each takes a time in seconds so it animates, or a fixed time for a still.

use crate::theme::Theme;
use ratatui::text::{Line, Span};

/// Ease out: fast, then settles. `f` from 0 to 1.
pub fn ease_out(f: f64) -> f64 {
    let f = f.clamp(0.0, 1.0);
    1.0 - (1.0 - f).powi(3)
}

/// The step lane across the top: `●────●────◉────○`. While `glide` is `Some((from, f))` the
/// marker slides from the `from` stop to the current one (`f` from 0 to 1).
pub fn step_lane(
    theme: &Theme,
    current: usize,
    total: usize,
    segment: usize,
    glide: Option<(usize, f64)>,
) -> Line<'static> {
    let stop_x = |i: usize| i * (segment + 1);
    let target = stop_x(current) as f64;
    let marker = match glide {
        Some((from, f)) if f < 1.0 => {
            let from = stop_x(from) as f64;
            (from + (target - from) * ease_out(f)).round() as usize
        }
        _ => stop_x(current),
    };
    let len = stop_x(total - 1) + 1;
    let spans = (0..len)
        .map(|x| {
            if x == marker {
                Span::styled("◉", theme.accent_strong())
            } else if x % (segment + 1) == 0 {
                if x < marker {
                    Span::styled("●", theme.accent())
                } else {
                    Span::styled("○", theme.muted())
                }
            } else if x < marker {
                Span::styled("─", theme.accent())
            } else {
                Span::styled("─", theme.muted())
            }
        })
        .collect::<Vec<_>>();
    Line::from(spans)
}

/// A tiny lane with a dot running along it: shown while a check is still running.
pub fn runner(theme: &Theme, t: f64, width: usize) -> Vec<Span<'static>> {
    let pos = if t.is_finite() {
        ((t * 7.0) as usize) % width
    } else {
        width / 2
    };
    vec![
        Span::styled("─".repeat(pos), theme.muted()),
        Span::styled("●", theme.accent()),
        Span::styled("─".repeat(width - 1 - pos), theme.muted()),
    ]
}

/// Autter's otter in its captain's hat, four lines tall. It blinks now and then.
pub fn otter(theme: &Theme, t: f64) -> Vec<Line<'static>> {
    let blink = t.is_finite() && (t % 3.6) < 0.16;
    let face = if blink { "=(-.-)=" } else { "=(o.o)=" };
    vec![
        Line::from(Span::styled("   ___", theme.hat())),
        Line::from(Span::styled(" _[___]_", theme.hat())),
        Line::from(Span::styled(format!(" {face}"), theme.otter())),
        Line::from(Span::styled("  (\")(\")", theme.otter())),
    ]
}

/// How the otter moves along its water lane.
#[derive(Debug, Clone, Copy)]
pub enum Swim {
    /// Back and forth across the lane, forever.
    Loop,
    /// From the start to the far end, arriving `secs` after `from`, then bobbing there.
    Arrive { from: f64, secs: f64 },
}

const SWIMMER: &str = "=(o.o)=";

/// The otter swimming along a water lane `width` columns wide, with a wake behind it.
pub fn swimming_otter(theme: &Theme, t: f64, width: usize, swim: Swim) -> Line<'static> {
    let width = width.max(SWIMMER.len() + 4);
    let room = width - SWIMMER.len() - 2;
    let pos = match (swim, t.is_finite()) {
        (Swim::Loop, true) => {
            let phase = (t * 0.12).fract();
            let tri = if phase < 0.5 {
                phase * 2.0
            } else {
                2.0 - phase * 2.0
            };
            (ease_out(tri) * room as f64).round() as usize
        }
        (Swim::Loop, false) => room / 3,
        (Swim::Arrive { from, secs }, true) => {
            (ease_out((t - from) / secs) * room as f64).round() as usize
        }
        (Swim::Arrive { .. }, false) => room,
    };
    // Ripples drift backwards; the two cells behind the otter are its wake.
    let shift = if t.is_finite() { (t * 4.0) as usize } else { 0 };
    let water = |range: std::ops::Range<usize>| -> String {
        range
            .map(|x| if (x + shift) % 3 == 0 { '~' } else { ' ' })
            .collect()
    };
    let wake_from = pos.saturating_sub(2);
    vec![
        Span::styled(water(0..wake_from), theme.water()),
        Span::styled("≈".repeat(pos - wake_from), theme.water()),
        Span::styled(SWIMMER, theme.otter()),
        Span::styled(water(pos + SWIMMER.len()..width), theme.water()),
    ]
    .into()
}

/// The install count as a lane: a packet travels to it when on; the lane is cut when off.
pub fn privacy_flow(theme: &Theme, t: f64, on: bool) -> Line<'static> {
    const L: usize = 22;
    let mut spans = vec![Span::styled("your machine ", theme.muted())];
    if on {
        let pos = if t.is_finite() {
            (((t * 0.6).fract()) * (L - 1) as f64) as usize
        } else {
            L / 2
        };
        spans.push(Span::styled("━".repeat(pos), theme.accent()));
        spans.push(Span::styled("●", theme.accent_strong()));
        spans.push(Span::styled("━".repeat(L - 1 - pos), theme.accent()));
        spans.push(Span::styled("▸ ", theme.accent()));
    } else {
        spans.push(Span::styled("─ ─ ─ ─ ─ ", theme.muted()));
        spans.push(Span::styled("✕", theme.muted()));
        spans.push(Span::styled(" ─ ─ ─ ─ ─ ", theme.muted()));
    }
    spans.push(Span::styled(" install count", theme.muted()));
    Line::from(spans)
}

/// One agent lane: a label, a track with a moving dot and the agent's name.
pub struct LaneRow {
    pub label: &'static str,
    pub agent: &'static str,
    /// Laps per second.
    pub speed: f64,
    /// Where the dot starts, 0 to 1.
    pub offset: f64,
    pub lead: bool,
}

pub const HERO_LANES: [LaneRow; 4] = [
    LaneRow {
        label: "01 fix-auth",
        agent: "Claude Code",
        speed: 0.11,
        offset: 0.15,
        lead: true,
    },
    LaneRow {
        label: "02 add-tests",
        agent: "Codex",
        speed: 0.08,
        offset: 0.55,
        lead: false,
    },
    LaneRow {
        label: "03 new-billing",
        agent: "Cursor",
        speed: 0.13,
        offset: 0.30,
        lead: false,
    },
    LaneRow {
        label: "04 refactor-db",
        agent: "Gemini CLI",
        speed: 0.09,
        offset: 0.80,
        lead: false,
    },
];

/// The four lanes of the welcome screen, `width` columns wide.
pub fn hero_lanes(theme: &Theme, t: f64, width: usize) -> Vec<Line<'static>> {
    let label_w = 16;
    let agent_w = 12;
    let track = width.saturating_sub(label_w + agent_w + 2).clamp(10, 64);
    HERO_LANES
        .iter()
        .map(|lane| {
            let f = if t.is_finite() {
                (lane.offset + t * lane.speed).fract()
            } else {
                lane.offset
            };
            let pos = ((f * (track - 1) as f64).round() as usize).min(track - 1);
            let trail_style = if lane.lead {
                theme.accent()
            } else {
                theme.text()
            };
            let dot_style = if lane.lead {
                theme.accent_strong()
            } else {
                theme.strong()
            };
            Line::from(vec![
                Span::styled(format!("{:<label_w$}", lane.label), theme.text()),
                Span::styled("━".repeat(pos), trail_style),
                Span::styled("●", dot_style),
                Span::styled("─".repeat(track - 1 - pos), theme.muted()),
                Span::styled(format!("  {:<agent_w$}", lane.agent), theme.muted()),
            ])
        })
        .collect()
}

fn track(
    theme: &Theme,
    len: usize,
    pos: Option<usize>,
    lead: bool,
    dim: bool,
) -> Vec<Span<'static>> {
    let trail = if dim {
        theme.muted()
    } else if lead {
        theme.accent()
    } else {
        theme.text()
    };
    let dot = if dim {
        theme.muted()
    } else if lead {
        theme.accent_strong()
    } else {
        theme.strong()
    };
    match pos {
        Some(p) => {
            let p = p.min(len - 1);
            vec![
                Span::styled("━".repeat(p), trail),
                Span::styled("●", dot),
                Span::styled("─".repeat(len - 1 - p), theme.muted()),
            ]
        }
        None => vec![Span::styled(
            "─".repeat(len),
            if dim { theme.muted() } else { trail },
        )],
    }
}

/// A position along a track that eases forward and holds, looping every `period` seconds.
fn travel(t: f64, period: f64, from: f64, to: f64, len: usize) -> usize {
    if !t.is_finite() {
        // A still frame (animation off) shows where each motion ends.
        return (to * (len - 1) as f64).round() as usize;
    }
    let phase = (t % period) / period;
    let eased = if phase < 0.7 {
        1.0 - (1.0 - phase / 0.7).powi(3)
    } else {
        1.0
    };
    ((from + (to - from) * eased) * (len - 1) as f64).round() as usize
}

/// The explainer picture for `step` (0 to 3). `width` drops the trailing labels when narrow.
pub fn explainer(theme: &Theme, step: usize, t: f64, width: u16) -> Vec<Line<'static>> {
    let wide = width >= 72;
    const L: usize = 18;
    let names = ["fix-auth   ", "add-tests  ", "new-billing"];
    let urls = [
        "fix-auth.localhost :3001",
        "add-tests.localhost :3002",
        "new-billing.localhost :3003",
    ];
    let m = |s: &'static str| Span::styled(s, theme.muted());
    let x = |s: &'static str| Span::styled(s, theme.text());
    match step {
        0 => vec![
            Line::default(),
            Line::from(vec![
                x("your-repo "),
                Span::styled("●", theme.strong()),
                Span::styled("━".repeat(32), theme.text()),
            ]),
            Line::from(vec![m("          .git  node_modules/  .env")]),
            Line::default(),
            Line::from(vec![m("          one folder, one copy of your app")]),
        ],
        1 => {
            let positions = [
                travel(t, 4.0, 0.1, 0.55, L),
                travel(t, 4.6, 0.05, 0.45, L),
                travel(t, 3.8, 0.15, 0.6, L),
            ];
            let branch = ["╭─ ", "┼─ ", "╰─ "];
            (0..3)
                .map(|i| {
                    let mut spans = vec![
                        if i == 1 {
                            x("your-repo ●━")
                        } else {
                            x("            ")
                        },
                        Span::styled(branch[i], theme.text()),
                        Span::styled(names[i], theme.text()),
                        Span::raw(" "),
                    ];
                    spans.extend(track(theme, L, Some(positions[i]), i == 0, false));
                    if wide {
                        spans.push(Span::styled(format!("  {}", urls[i]), theme.muted()));
                    }
                    Line::from(spans)
                })
                .collect()
        }
        2 => {
            let meet = travel(t, 3.0, 0.2, 0.6, L);
            let pulse = (t * 2.0).fract() < 0.6;
            let mut lines = Vec::new();
            for i in 0..3 {
                let pos = if i < 2 {
                    meet
                } else {
                    (L as f64 * 0.4) as usize
                };
                let mut spans = vec![
                    if i == 1 {
                        x("your-repo ●━")
                    } else {
                        x("            ")
                    },
                    Span::styled(["╭─ ", "┼─ ", "╰─ "][i], theme.text()),
                    Span::styled(names[i], theme.text()),
                    Span::raw(" "),
                ];
                spans.extend(track(theme, L, Some(pos), i == 0, i == 2));
                lines.push(Line::from(spans));
                if i == 0 {
                    let tag = if pulse {
                        theme.warn().add_modifier(ratatui::style::Modifier::BOLD)
                    } else {
                        theme.warn()
                    };
                    lines.push(Line::from(vec![
                        Span::raw(" ".repeat(14)),
                        Span::styled("! both editing validateSession()", tag),
                    ]));
                }
            }
            lines.push(Line::default());
            lines
        }
        _ => {
            let pos = travel(t, 3.4, 0.6, 1.0, L);
            let done = pos >= L - 1;
            let mut first = vec![x("            "), x("╭─ "), x(names[0]), Span::raw(" ")];
            first.extend(track(theme, L, Some(pos), true, false));
            first.push(Span::styled(
                "╮",
                if done { theme.accent() } else { theme.muted() },
            ));
            let mut second = vec![
                x("your-repo ●━"),
                Span::styled("┼─ ", theme.muted()),
                Span::styled(names[1], theme.muted()),
                Span::raw(" "),
            ];
            second.extend(track(theme, L, None, false, true));
            if done {
                second.push(Span::styled("╰─", theme.accent()));
                second.push(Span::styled("● PR", theme.accent_strong()));
                if wide {
                    second.push(Span::styled("  one clean pull request", theme.text()));
                }
            }
            let mut third = vec![
                x("            "),
                Span::styled("╰─ ", theme.muted()),
                Span::styled(names[2], theme.muted()),
                Span::raw(" "),
            ];
            third.extend(track(theme, L, None, false, true));
            vec![
                Line::default(),
                Line::from(first),
                Line::from(second),
                Line::from(third),
                Line::default(),
            ]
        }
    }
}

/// "Lanes." in big block letters, four rows tall. A small bold sans drawn for these six glyphs
/// (eight pixels tall, two-pixel strokes), rendered with half blocks so each character cell holds
/// two pixel rows. It sweeps in from the left over the first `reveal` seconds; the stop is mint.
pub fn wordmark(theme: &Theme, t: f64, reveal: f64) -> Vec<Line<'static>> {
    // Each glyph: 8 pixel rows, '#' set. The last flag marks the accent full stop.
    const GLYPHS: [(&[&str; 8], bool); 6] = [
        (
            &[
                "##   ", "##   ", "##   ", "##   ", "##   ", "##   ", "##   ", "#####",
            ],
            false,
        ),
        (
            &[
                "      ", "      ", " #### ", "    ##", " #####", "##  ##", "##  ##", " #####",
            ],
            false,
        ),
        (
            &[
                "      ", "      ", "##### ", "##  ##", "##  ##", "##  ##", "##  ##", "##  ##",
            ],
            false,
        ),
        (
            &[
                "      ", "      ", " #### ", "##  ##", "######", "##    ", "##    ", " #####",
            ],
            false,
        ),
        (
            &[
                "      ", "      ", " #####", "##    ", " #### ", "    ##", "    ##", "##### ",
            ],
            false,
        ),
        (&["  ", "  ", "  ", "  ", "  ", "  ", "##", "##"], true),
    ];
    // Pixel columns across the whole word, one blank column between glyphs.
    let mut cols: Vec<([bool; 8], bool)> = Vec::new();
    for (g, (rows, dot)) in GLYPHS.iter().enumerate() {
        let width = rows[0].len();
        for x in 0..width {
            let mut col = [false; 8];
            for (y, row) in rows.iter().enumerate() {
                col[y] = row.as_bytes()[x] == b'#';
            }
            cols.push((col, *dot));
        }
        if g + 1 < GLYPHS.len() {
            cols.push(([false; 8], false));
        }
    }
    let shown = if t.is_finite() {
        (ease_out(t / reveal) * cols.len() as f64).ceil() as usize
    } else {
        cols.len()
    };
    (0..4)
        .map(|row| {
            let spans = cols
                .iter()
                .enumerate()
                .map(|(x, (col, dot))| {
                    let ch = match (x < shown, col[row * 2], col[row * 2 + 1]) {
                        (false, _, _) | (true, false, false) => " ",
                        (true, true, true) => "█",
                        (true, true, false) => "▀",
                        (true, false, true) => "▄",
                    };
                    Span::styled(
                        ch,
                        if *dot {
                            theme.accent_strong()
                        } else {
                            theme.strong()
                        },
                    )
                })
                .collect::<Vec<_>>();
            Line::from(spans)
        })
        .collect()
}
