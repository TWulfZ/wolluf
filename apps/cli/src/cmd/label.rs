//! `wolluf label [stats|export]` over `LabelingService`: a line-based labelling session on
//! stdin/stdout, so it also runs scripted (tests pipe the answers in).

use std::io::{BufRead, Write};
use std::process::ExitCode;

use wolluf_app::clock::SystemClock;
use wolluf_app::context::AppContext;
use wolluf_app::errors::AppError;
use wolluf_app::features::labeling::dto::{
    AnchorDto, CountDto, LabelExportDto, LabelStatsDto, LabelSubmitDto, LabelWindowDto,
    PatternDefDto, SampleRequestDto, ThumbPrefDto, WindowOpDto,
};
use wolluf_app::features::labeling::keys;
use wolluf_core::Clock;

use crate::cli::{LabelArgs, LabelCmd, LabelSessionArgs};
use crate::exit;
use crate::render;

const MS_PER_SECOND: i32 = 1_000;
const SECONDS_PER_MINUTE: i32 = 60;
const PROMPT: &str = "> ";

const NO_PATTERN: &str = "x";
const THUMB_LEFT: &str = "tl";
const THUMB_RIGHT: &str = "tr";
const COMMANDS: &str = "x no clear pattern · tl/tr thumb side · m mixed · ? unsure · s skip · \
                        u undo · w+/w- window ±1 s · n/p shift ½ window · h help · q quit";
const HELP: &str = "\
Type the patterns you see, as short keys or full ids, separated by spaces or commas, then
Enter to save the window and move on. `x` answers \"no clear pattern\" and is stored, unlike
a skip. `m` and `?` toggle the mixed and unsure flags, `tl`/`tr` mark the window as feeling
better with the left or right thumb (the same side again clears it); all four work alone or
inside a label line. Other commands, alone on a line:
  s      skip this window (nothing is stored)
  u      undo the last label saved in this session
  w+/w-  widen or narrow the window by 1 s
  n/p    shift the window forward or back by half its length
  h      this help
  q      quit and print the session counts";

pub(crate) async fn run(ctx: &AppContext, args: LabelArgs, json: bool) -> anyhow::Result<ExitCode> {
    match args.command {
        Some(LabelCmd::Stats) => {
            let stats = ctx.labeling().stats().await?;
            if json {
                render::json(&stats)?;
            } else {
                render::text(&stats_text(&stats))?;
            }
        }
        Some(LabelCmd::Export { out }) => {
            let done = ctx.labeling().export_to(out).await?;
            if json {
                render::json(&done)?;
            } else {
                render::text(&export_text(&done))?;
            }
        }
        None => {
            // The main thread only drives the runtime, so blocking it on stdin starves nothing.
            let stdin = std::io::stdin();
            let stdout = std::io::stdout();
            session(ctx, args.session, &mut stdin.lock(), &mut stdout.lock()).await?;
        }
    }
    Ok(exit::exit_code(exit::SUCCESS))
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Input {
    /// Pattern tokens as typed; the labeling service resolves them.
    Labels {
        tokens: Vec<String>,
        /// `x`: the stored "no clear pattern" answer; `tokens` is then empty.
        no_pattern: bool,
        toggle_mixed: bool,
        toggle_unsure: bool,
        /// Set inline for this answer, overriding the window's side.
        thumb: Option<ThumbPrefDto>,
    },
    Thumb(ThumbPrefDto),
    Mixed,
    Unsure,
    Skip,
    Undo,
    Reshape(WindowOpDto),
    Help,
    Quit,
}

fn command(token: &str) -> Option<Input> {
    Some(match token {
        "m" => Input::Mixed,
        "?" => Input::Unsure,
        "s" => Input::Skip,
        "u" => Input::Undo,
        "w+" => Input::Reshape(WindowOpDto::Widen),
        "w-" => Input::Reshape(WindowOpDto::Narrow),
        "n" => Input::Reshape(WindowOpDto::Next),
        "p" => Input::Reshape(WindowOpDto::Prev),
        "h" => Input::Help,
        "q" => Input::Quit,
        THUMB_LEFT => Input::Thumb(ThumbPrefDto::Left),
        THUMB_RIGHT => Input::Thumb(ThumbPrefDto::Right),
        _ => return None,
    })
}

/// One answer line. Errors are messages for the user, who is then asked again.
fn parse_input(line: &str) -> Result<Input, String> {
    let lower = line.to_lowercase();
    let words: Vec<&str> = lower
        .split(|c: char| c.is_whitespace() || c == ',')
        .filter(|t| !t.is_empty())
        .collect();
    match words.as_slice() {
        [] => return Err("empty answer; type h for help".to_owned()),
        [one] => {
            if let Some(cmd) = command(one) {
                return Ok(cmd);
            }
        }
        _ => {}
    }
    let mut tokens: Vec<String> = Vec::new();
    let (mut no_pattern, mut toggle_mixed, mut toggle_unsure) = (false, false, false);
    let mut thumb: Option<ThumbPrefDto> = None;
    for word in words {
        match word {
            "m" => toggle_mixed = !toggle_mixed,
            "?" => toggle_unsure = !toggle_unsure,
            NO_PATTERN => no_pattern = true,
            THUMB_LEFT | THUMB_RIGHT => {
                let side = if word == THUMB_LEFT {
                    ThumbPrefDto::Left
                } else {
                    ThumbPrefDto::Right
                };
                if thumb.is_some_and(|t| t != side) {
                    return Err("choose one thumb side".to_owned());
                }
                thumb = Some(side);
            }
            _ if command(word).is_some() => {
                return Err(format!("`{word}` is a command; send it on its own line"));
            }
            _ => tokens.push(word.to_owned()),
        }
    }
    if no_pattern && !tokens.is_empty() {
        return Err("`x` means no clear pattern; send it without patterns".to_owned());
    }
    if !no_pattern && tokens.is_empty() {
        return Err("no pattern given".to_owned());
    }
    Ok(Input::Labels {
        tokens,
        no_pattern,
        toggle_mixed,
        toggle_unsure,
        thumb,
    })
}

/// The session's wording for the labeling slice's keyed errors; anything else prints as the
/// standard error line.
fn message(e: &AppError) -> String {
    let arg = |k: &str| e.args.get(k).map_or("", String::as_str);
    match e.message_key.as_ref() {
        keys::UNKNOWN_PATTERN => format!("error: unknown pattern `{}`", arg("pattern")),
        keys::WINDOW_TOO_SHORT => {
            let seconds = arg("minMs").parse::<f64>().unwrap_or(0.0) / f64::from(MS_PER_SECOND);
            format!("error: the window cannot be shorter than {seconds} s")
        }
        _ => render::error_line(e),
    }
}

enum Undo {
    Nothing,
    Done(String),
    Failed(AppError),
}

/// The last saved id leaves the stack only once its undo is stored, so a failed undo can be
/// retried.
async fn undo_last(
    saved: &mut Vec<String>,
    undo: impl AsyncFnOnce(&str) -> Result<(), AppError>,
) -> Undo {
    let Some(id) = saved.last().cloned() else {
        return Undo::Nothing;
    };
    match undo(&id).await {
        Ok(()) => {
            saved.pop();
            Undo::Done(id)
        }
        Err(e) => Undo::Failed(e),
    }
}

#[derive(Debug, Default)]
struct Counts {
    labelled: u32,
    skipped: u32,
    undone: u32,
}

#[derive(Debug, Default, Clone, Copy)]
struct Flags {
    mixed: bool,
    unsure: bool,
    thumb: Option<ThumbPrefDto>,
}

impl Flags {
    fn text(self) -> String {
        let mut parts: Vec<&str> = Vec::new();
        if self.mixed {
            parts.push("mixed");
        }
        if self.unsure {
            parts.push("unsure");
        }
        match self.thumb {
            Some(ThumbPrefDto::Left) => parts.push("thumb:left"),
            Some(ThumbPrefDto::Right) => parts.push("thumb:right"),
            None => {}
        }
        if parts.is_empty() {
            "-".to_owned()
        } else {
            parts.join(" ")
        }
    }

    fn toggled_thumb(self, side: ThumbPrefDto) -> Self {
        Self {
            thumb: (self.thumb != Some(side)).then_some(side),
            ..self
        }
    }
}

async fn session(
    ctx: &AppContext,
    args: LabelSessionArgs,
    input: &mut impl BufRead,
    out: &mut impl Write,
) -> anyhow::Result<()> {
    let seed = args
        .seed
        .unwrap_or_else(|| u64::try_from(SystemClock.now().0).unwrap_or(0));
    let taxonomy = ctx.labeling().taxonomy(args.keys)?;
    writeln!(out, "seed {seed} (replay with --seed {seed})")?;
    let mut shown: Vec<AnchorDto> = Vec::new();
    let mut saved: Vec<String> = Vec::new();
    let mut counts = Counts::default();
    let mut round: u32 = 0;
    'rounds: loop {
        let request = SampleRequestDto {
            keymode: args.keys,
            seed: seed.to_string(),
            round,
            window_ms: Some(args.window_ms),
            scale: args.scale.clone(),
            level_min: args.level_min,
            level_max: args.level_max,
            exclude: shown.clone(),
        };
        let Some(window) = ctx.labeling().sample(request).await? else {
            writeln!(out, "no window left to label")?;
            break;
        };
        shown.push(window.anchor.clone());
        let mut anchor = window.anchor.clone();
        let mut flags = Flags::default();
        let view = |anchor: &AnchorDto, flags: Flags, labelled: u32| View {
            round,
            labelled,
            window: window.clone(),
            anchor: anchor.clone(),
            flags,
        };
        draw(ctx, out, &view(&anchor, flags, counts.labelled), &taxonomy).await?;
        loop {
            write!(out, "{PROMPT}")?;
            out.flush()?;
            let mut line = String::new();
            if input.read_line(&mut line)? == 0 {
                writeln!(out)?;
                break 'rounds;
            }
            let parsed = match parse_input(&line) {
                Ok(parsed) => parsed,
                Err(message) => {
                    writeln!(out, "error: {message}")?;
                    continue;
                }
            };
            match parsed {
                Input::Quit => break 'rounds,
                Input::Skip => {
                    counts.skipped += 1;
                    break;
                }
                Input::Help => writeln!(out, "{HELP}")?,
                Input::Mixed => {
                    flags.mixed = !flags.mixed;
                    writeln!(out, "flags: {}", flags.text())?;
                }
                Input::Unsure => {
                    flags.unsure = !flags.unsure;
                    writeln!(out, "flags: {}", flags.text())?;
                }
                Input::Thumb(side) => {
                    flags = flags.toggled_thumb(side);
                    writeln!(out, "flags: {}", flags.text())?;
                }
                Input::Undo => {
                    let labeling = ctx.labeling();
                    match undo_last(&mut saved, async |id| labeling.undo(id).await).await {
                        Undo::Nothing => writeln!(out, "error: nothing to undo in this session")?,
                        Undo::Done(id) => {
                            counts.undone += 1;
                            writeln!(out, "undone {id}")?;
                        }
                        Undo::Failed(e) => writeln!(out, "{}", message(&e))?,
                    }
                }
                Input::Reshape(op) => match ctx.labeling().reshape(anchor.clone(), op).await {
                    Ok(next) => {
                        anchor = next;
                        let v = view(&anchor, flags, counts.labelled);
                        draw(ctx, out, &v, &taxonomy).await?;
                    }
                    Err(e) => writeln!(out, "{}", message(&e))?,
                },
                Input::Labels {
                    tokens,
                    no_pattern,
                    toggle_mixed,
                    toggle_unsure,
                    thumb,
                } => {
                    let patterns = if no_pattern {
                        Vec::new()
                    } else {
                        match ctx.labeling().resolve_patterns(args.keys, &tokens) {
                            Ok(ids) => ids.iter().map(ToString::to_string).collect(),
                            Err(e) => {
                                writeln!(out, "{}", message(&e))?;
                                continue;
                            }
                        }
                    };
                    let submit = LabelSubmitDto {
                        anchor: anchor.clone(),
                        patterns,
                        no_pattern,
                        mixed: flags.mixed != toggle_mixed,
                        unsure: flags.unsure != toggle_unsure,
                        thumb_pref: thumb.or(flags.thumb),
                    };
                    match ctx.labeling().submit(submit).await {
                        Ok(event) => {
                            counts.labelled += 1;
                            writeln!(out, "saved {}", event.id)?;
                            saved.push(event.id);
                            break;
                        }
                        Err(e) => writeln!(out, "{}", message(&e))?,
                    }
                }
            }
        }
        round = round.saturating_add(1);
    }
    let total = ctx.labeling().stats().await?.total;
    writeln!(
        out,
        "session: labelled {}, skipped {}, undone {} · gold set total {total}",
        counts.labelled, counts.skipped, counts.undone
    )?;
    Ok(())
}

/// What one redraw shows.
struct View {
    round: u32,
    labelled: u32,
    window: LabelWindowDto,
    anchor: AnchorDto,
    flags: Flags,
}

async fn draw(
    ctx: &AppContext,
    out: &mut impl Write,
    view: &View,
    taxonomy: &[PatternDefDto],
) -> anyhow::Result<()> {
    let View {
        round,
        labelled,
        window,
        anchor,
        flags,
    } = view;
    let field = ctx
        .library()
        .render(&anchor.md5, anchor.t0_ms, anchor.t1_ms, None)
        .await?;
    writeln!(
        out,
        "\n── round {} · labelled {labelled} this session ──",
        round + 1,
    )?;
    let mut about = format!("{} [{}]", window.title, window.version);
    if let Some(level) = &window.level {
        about.push_str(&format!(" · {level}"));
    }
    about.push_str(&format!(" · {}", window.stratum));
    if window.played {
        about.push_str(" · played");
    }
    writeln!(out, "{about}")?;
    writeln!(
        out,
        "{}  {}–{} ({:.1} s)  flags: {}",
        anchor.md5,
        clock(anchor.t0_ms),
        clock(anchor.t1_ms),
        f64::from(anchor.t1_ms - anchor.t0_ms) / f64::from(MS_PER_SECOND),
        flags.text()
    )?;
    write!(out, "{field}")?;
    write!(out, "{}", legend(taxonomy))?;
    writeln!(out, "{COMMANDS}")?;
    Ok(())
}

/// `mm:ss.mmm`.
fn clock(ms: i32) -> String {
    let sign = if ms < 0 { "-" } else { "" };
    let ms = ms.unsigned_abs();
    let per_minute = (MS_PER_SECOND * SECONDS_PER_MINUTE).unsigned_abs();
    let per_second = MS_PER_SECOND.unsigned_abs();
    format!(
        "{sign}{:02}:{:02}.{:03}",
        ms / per_minute,
        ms / per_second % SECONDS_PER_MINUTE.unsigned_abs(),
        ms % per_second
    )
}

/// One line per axis, in taxonomy order: `key name` pairs, the name being the id's last part.
fn legend(taxonomy: &[PatternDefDto]) -> String {
    let mut axes: Vec<(&str, Vec<String>)> = Vec::new();
    for p in taxonomy {
        let name = p.id.rsplit('.').next().unwrap_or(&p.id);
        let entry = format!("{} {name}", p.key);
        match axes.iter_mut().find(|(axis, _)| *axis == p.axis) {
            Some((_, entries)) => entries.push(entry),
            None => axes.push((&p.axis, vec![entry])),
        }
    }
    let width = axes.iter().map(|(a, _)| a.len()).max().unwrap_or(0);
    axes.iter()
        .map(|(axis, entries)| format!("{axis:<width$}  {}\n", entries.join("  ")))
        .collect()
}

fn counts_table(title: &str, rows: &[CountDto]) -> String {
    let rows: Vec<Vec<String>> = rows
        .iter()
        .map(|c| vec![c.key.clone(), c.count.to_string()])
        .collect();
    render::table(&[title, "COUNT"], &rows)
}

fn stats_text(s: &LabelStatsDto) -> String {
    let mut text = render::key_values(&[
        ("total", s.total.to_string()),
        ("no pattern", s.no_pattern.to_string()),
        ("mixed", s.mixed.to_string()),
        ("unsure", s.unsure.to_string()),
        ("thumb left", s.thumb_left.to_string()),
        ("thumb right", s.thumb_right.to_string()),
    ]);
    for (title, rows) in [
        ("PATTERN", &s.per_pattern),
        ("AXIS", &s.per_axis),
        ("STRATUM", &s.per_stratum),
    ] {
        text.push('\n');
        text.push_str(&counts_table(title, rows));
    }
    text
}

fn export_text(d: &LabelExportDto) -> String {
    render::key_values(&[("path", d.path.clone()), ("rows", d.rows.to_string())])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn labels(tokens: &[&str], mixed: bool, unsure: bool) -> Input {
        Input::Labels {
            tokens: tokens.iter().map(|p| (*p).to_owned()).collect(),
            no_pattern: false,
            toggle_mixed: mixed,
            toggle_unsure: unsure,
            thumb: None,
        }
    }

    /// Tokens a pattern key may never be.
    fn reserved(token: &str) -> bool {
        command(token).is_some() || token == NO_PATTERN
    }

    #[test]
    fn no_pattern_and_thumb_side_tokens() {
        let none = |mixed, thumb| Input::Labels {
            tokens: vec![],
            no_pattern: true,
            toggle_mixed: mixed,
            toggle_unsure: false,
            thumb,
        };
        assert_eq!(parse_input("x"), Ok(none(false, None)));
        assert_eq!(
            parse_input("x tl m"),
            Ok(none(true, Some(ThumbPrefDto::Left)))
        );
        assert_eq!(
            parse_input("js tr"),
            Ok(Input::Labels {
                tokens: vec!["js".to_owned()],
                no_pattern: false,
                toggle_mixed: false,
                toggle_unsure: false,
                thumb: Some(ThumbPrefDto::Right),
            })
        );
        assert_eq!(parse_input("tl"), Ok(Input::Thumb(ThumbPrefDto::Left)));
        assert_eq!(parse_input("TR"), Ok(Input::Thumb(ThumbPrefDto::Right)));
        assert_eq!(
            parse_input("x js"),
            Err("`x` means no clear pattern; send it without patterns".to_owned())
        );
        assert_eq!(
            parse_input("js tl tr"),
            Err("choose one thumb side".to_owned())
        );
    }

    #[test]
    fn flags_text_includes_the_thumb_side() {
        let flags = Flags {
            mixed: true,
            unsure: false,
            thumb: Some(ThumbPrefDto::Right),
        };
        assert_eq!(flags.text(), "mixed thumb:right");
        assert_eq!(Flags::default().text(), "-");
        assert_eq!(
            Flags {
                thumb: Some(ThumbPrefDto::Left),
                ..Flags::default()
            }
            .toggled_thumb(ThumbPrefDto::Left)
            .thumb,
            None,
            "the same side again goes back to neutral"
        );
    }

    #[test]
    fn label_lines_become_tokens_and_toggles() {
        assert_eq!(parse_input("js\n"), Ok(labels(&["js"], false, false)));
        assert_eq!(
            parse_input(" CJ,js , cj "),
            Ok(labels(&["cj", "js", "cj"], false, false)),
            "resolution and dedup belong to the app"
        );
        assert_eq!(
            parse_input("a m ? m js"),
            Ok(labels(&["a", "js"], false, true))
        );
    }

    #[test]
    fn commands_stand_alone() {
        for (line, want) in [
            ("m", Input::Mixed),
            ("?", Input::Unsure),
            ("s", Input::Skip),
            ("u", Input::Undo),
            ("w+", Input::Reshape(WindowOpDto::Widen)),
            ("w-", Input::Reshape(WindowOpDto::Narrow)),
            ("n", Input::Reshape(WindowOpDto::Next)),
            ("p", Input::Reshape(WindowOpDto::Prev)),
            ("h", Input::Help),
            ("Q\n", Input::Quit),
        ] {
            assert_eq!(parse_input(line), Ok(want), "{line:?}");
        }
    }

    #[test]
    fn bad_answers_are_messages() {
        assert_eq!(
            parse_input("js s"),
            Err("`s` is a command; send it on its own line".to_owned())
        );
        assert_eq!(parse_input("m ?"), Err("no pattern given".to_owned()));
        assert!(parse_input("  \n").is_err());
    }

    #[test]
    fn app_errors_read_as_session_messages() {
        use wolluf_app::features::labeling::keys;
        let unknown = AppError::invalid_input()
            .with_key(keys::UNKNOWN_PATTERN)
            .with_arg("pattern", "zz");
        assert_eq!(message(&unknown), "error: unknown pattern `zz`");
        let short = AppError::invalid_input()
            .with_key(keys::WINDOW_TOO_SHORT)
            .with_arg("minMs", "1000");
        assert_eq!(
            message(&short),
            "error: the window cannot be shorter than 1 s"
        );
        assert_eq!(
            message(&AppError::conflict()),
            "error[CONFLICT]: error.code.CONFLICT"
        );
    }

    #[tokio::test]
    async fn undo_pops_only_after_success() {
        let mut saved = vec!["a".to_owned(), "b".to_owned()];
        let failed = undo_last(&mut saved, async |_| Err(AppError::conflict())).await;
        assert!(matches!(failed, Undo::Failed(_)));
        assert_eq!(saved, ["a", "b"], "a failed undo keeps the id");
        let done = undo_last(&mut saved, async |id| {
            assert_eq!(id, "b");
            Ok(())
        })
        .await;
        assert!(matches!(done, Undo::Done(ref id) if id == "b"));
        assert_eq!(saved, ["a"]);
        let mut empty = Vec::new();
        assert!(matches!(
            undo_last(&mut empty, async |_| Ok(())).await,
            Undo::Nothing
        ));
    }

    #[test]
    fn no_pattern_key_shadows_a_command() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = wolluf_app::context::AppContext::open(
            wolluf_app::context::AppPaths::from_data_dir(dir.path().join("data")),
            std::sync::Arc::new(SystemClock),
        )
        .unwrap();
        let keys = ctx.labeling().taxonomy(7).unwrap();
        for p in &keys {
            assert!(!reserved(&p.key), "{} shadows a command or answer", p.key);
            assert!(!p.key.contains(['?', ',', ' ', '+', '-']), "{}", p.key);
        }
    }

    #[test]
    fn clock_and_legend() {
        assert_eq!(clock(0), "00:00.000");
        assert_eq!(clock(65_250), "01:05.250");
        let def = |id: &str, key: &str| PatternDefDto {
            id: id.to_owned(),
            axis: format!("7k.{}", id.rsplit_once('.').unwrap().0),
            key: key.to_owned(),
            description: String::new(),
        };
        let legend = legend(&[
            def("regular.stream.jumpstream", "js"),
            def("regular.jack.chordjack", "cj"),
            def("regular.jack.anchor", "a"),
        ]);
        assert_eq!(
            legend,
            "7k.regular.stream  js jumpstream\n7k.regular.jack    cj chordjack  a anchor\n"
        );
    }
}
