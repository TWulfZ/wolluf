//! Blind gold-set sampling: which chart window the labeller sees next.
//!
//! Charts fall into strata by their labelled level and their density, and rounds visit the
//! strata in turn, so a few hundred labels spread over the whole difficulty range instead of
//! following the library's shape. Everything here is a pure function of the seed, the round,
//! the charts and the anchors to avoid; the caller supplies row times.

use std::collections::BTreeMap;
use std::fmt;

use wolluf_core::{ChartMd5, SegmentAnchor, TimeUs};
use wolluf_engine::labels::{LevelFamily, level_family};

use super::window;

/// D17: sampling thresholds, not inline numbers.
#[derive(Debug, Clone, PartialEq)]
pub struct SamplerParams {
    pub window: TimeUs,
    /// Ascending NPS bin edges; `n` edges make `n + 1` bins.
    pub nps_edges: Vec<f64>,
    /// A window with fewer rows than this is mostly a break.
    pub min_rows: usize,
    /// Charts tried in one stratum before the round moves to the next stratum.
    pub max_chart_tries: usize,
    /// How far `w+`/`w-` move the window end.
    pub reshape_step: TimeUs,
    /// Shortest window `w-` leaves.
    pub min_window: TimeUs,
}

impl Default for SamplerParams {
    fn default() -> Self {
        Self {
            // Long enough to show a pattern and its context, short enough for one screen.
            window: TimeUs::from_ms(4_000),
            // 7K dans run from ~10 NPS (1st) to 25+ (10th and up).
            nps_edges: vec![10.0, 16.0, 22.0],
            min_rows: 4,
            max_chart_tries: 8,
            reshape_step: TimeUs::from_ms(1_000),
            min_window: TimeUs::from_ms(1_000),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct LevelLabel {
    pub source: String,
    pub scale: String,
    pub level_ord: Option<f64>,
    pub level_text: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ChartFacts {
    pub md5: ChartMd5,
    pub nps: f64,
    pub played: bool,
    pub labels: Vec<LevelLabel>,
}

/// Quartile of a level within its own scale.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Tier {
    Low,
    Mid,
    High,
    VeryHigh,
}

impl Tier {
    const ALL: [Self; 4] = [Self::Low, Self::Mid, Self::High, Self::VeryHigh];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Mid => "mid",
            Self::High => "high",
            Self::VeryHigh => "very_high",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum LevelKey {
    /// Dan ordinals share one ladder across courses and practice packs.
    Dan(u8),
    Tier(LevelFamily, Tier),
    Unlevelled,
}

impl fmt::Display for LevelKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Dan(n) => write!(f, "dan_{n:02}"),
            Self::Tier(family, tier) => write!(f, "{}_{}", family.as_str(), tier.as_str()),
            Self::Unlevelled => f.write_str("unlevelled"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Stratum {
    pub level: LevelKey,
    pub nps_bin: u8,
}

impl fmt::Display for Stratum {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/nps_{}", self.level, self.nps_bin)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Assignment {
    pub stratum: Stratum,
    /// `scale:level_text` of the label that placed the chart, or of its first label when none
    /// has a level.
    pub label: Option<String>,
}

fn label_text(l: &LevelLabel) -> String {
    format!("{}:{}", l.scale, l.level_text)
}

fn nps_bin(nps: f64, edges: &[f64]) -> u8 {
    let bin = edges.iter().filter(|&&e| nps >= e).count();
    u8::try_from(bin).unwrap_or(u8::MAX)
}

/// Sorted levels of every BMS and O2Jam scale, for per-scale quartiles.
fn scale_levels(charts: &[ChartFacts]) -> BTreeMap<&str, Vec<f64>> {
    let mut out: BTreeMap<&str, Vec<f64>> = BTreeMap::new();
    for l in charts.iter().flat_map(|c| &c.labels) {
        let tiered = matches!(
            level_family(&l.source),
            Some(LevelFamily::Bms | LevelFamily::O2jam)
        );
        if let (true, Some(ord)) = (tiered, l.level_ord) {
            out.entry(l.scale.as_str()).or_default().push(ord);
        }
    }
    for levels in out.values_mut() {
        levels.sort_by(f64::total_cmp);
    }
    out
}

fn tier(levels: &[f64], ord: f64) -> Tier {
    let below = levels.partition_point(|&v| v < ord);
    let at = below * Tier::ALL.len() / levels.len().max(1);
    Tier::ALL.get(at).copied().unwrap_or(Tier::VeryHigh)
}

fn level_key(l: &LevelLabel, levels: &BTreeMap<&str, Vec<f64>>) -> Option<LevelKey> {
    let ord = l.level_ord?;
    match level_family(&l.source)? {
        LevelFamily::Dan => Some(LevelKey::Dan(ord.floor().clamp(0.0, 255.0) as u8)),
        family => {
            let levels = levels.get(l.scale.as_str())?;
            Some(LevelKey::Tier(family, tier(levels, ord)))
        }
    }
}

/// Every chart's stratum. Quartiles come from all given charts, so pass the whole library,
/// not a filtered subset, or stratum names shift with the filter.
pub fn assign(charts: &[ChartFacts], params: &SamplerParams) -> BTreeMap<ChartMd5, Assignment> {
    let levels = scale_levels(charts);
    charts
        .iter()
        .map(|c| {
            // Dan first: its ladder is the one the pilot's skill is read against.
            let placed = c
                .labels
                .iter()
                .filter_map(|l| Some((level_key(l, &levels)?, l)))
                .min_by(|(ka, la), (kb, lb)| {
                    let family = |k: &LevelKey| match k {
                        LevelKey::Dan(_) => 0,
                        LevelKey::Tier(f, _) => 1 + *f as u8,
                        LevelKey::Unlevelled => u8::MAX,
                    };
                    family(ka)
                        .cmp(&family(kb))
                        .then_with(|| la.scale.cmp(&lb.scale))
                        .then_with(|| ka.cmp(kb))
                });
            let (level, label) = match placed {
                Some((key, l)) => (key, Some(label_text(l))),
                None => (LevelKey::Unlevelled, c.labels.first().map(label_text)),
            };
            let stratum = Stratum {
                level,
                nps_bin: nps_bin(c.nps, &params.nps_edges),
            };
            (c.md5, Assignment { stratum, label })
        })
        .collect()
}

/// Label criteria, as in the library listing: a chart matches when one of its labels does.
/// Bounds are inclusive and exclude labels without a level.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct LevelFilter {
    pub scale: Option<String>,
    pub level_min: Option<f64>,
    pub level_max: Option<f64>,
}

impl LevelFilter {
    fn is_empty(&self) -> bool {
        self.scale.is_none() && self.level_min.is_none() && self.level_max.is_none()
    }

    fn matches(&self, l: &LevelLabel) -> bool {
        let scale = self.scale.as_ref().is_none_or(|s| s == &l.scale);
        let bound = |b: Option<f64>, ok: fn(f64, f64) -> bool| {
            b.is_none_or(|b| l.level_ord.is_some_and(|o| ok(o, b)))
        };
        scale && bound(self.level_min, |o, b| o >= b) && bound(self.level_max, |o, b| o <= b)
    }

    pub fn accepts(&self, labels: &[LevelLabel]) -> bool {
        self.is_empty() || labels.iter().any(|l| self.matches(l))
    }
}

/// Strata in order, each with its charts played first, then in a seed-shuffled order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pool {
    strata: Vec<(Stratum, Vec<ChartMd5>)>,
}

impl Pool {
    pub fn new(
        seed: u64,
        charts: &[ChartFacts],
        assigned: &BTreeMap<ChartMd5, Assignment>,
        filter: &LevelFilter,
    ) -> Self {
        let mut by_stratum: BTreeMap<Stratum, Vec<(bool, u64, ChartMd5)>> = BTreeMap::new();
        for c in charts.iter().filter(|c| filter.accepts(&c.labels)) {
            let Some(a) = assigned.get(&c.md5) else {
                continue;
            };
            let order = mix(seed, b"chart", &[&c.md5.0]);
            by_stratum
                .entry(a.stratum)
                .or_default()
                .push((!c.played, order, c.md5));
        }
        let strata = by_stratum
            .into_iter()
            .map(|(s, mut charts)| {
                charts.sort_unstable();
                (s, charts.into_iter().map(|(.., md5)| md5).collect())
            })
            .collect();
        Self { strata }
    }

    pub fn len(&self) -> usize {
        self.strata.len()
    }

    pub fn is_empty(&self) -> bool {
        self.strata.is_empty()
    }

    pub fn charts(&self) -> impl Iterator<Item = ChartMd5> + '_ {
        self.strata.iter().flat_map(|(_, c)| c.iter().copied())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pick {
    pub md5: ChartMd5,
    pub t0: TimeUs,
    pub t1: TimeUs,
    pub stratum: Stratum,
}

/// Charts to try for round `round`, in order, with their stratum. Round `r` goes to stratum
/// `(offset + r) mod n`; a stratum's `k`-th visit starts at its `k`-th chart, so played charts
/// come first and later visits move on. The next strata follow, in case this one has no free
/// window left.
pub fn plan(
    seed: u64,
    round: u32,
    pool: &Pool,
    params: &SamplerParams,
) -> Vec<(ChartMd5, Stratum)> {
    let n = pool.len();
    if n == 0 {
        return Vec::new();
    }
    let offset = usize::try_from(mix(seed, b"offset", &[]) % n as u64).unwrap_or(0);
    let mut out = Vec::new();
    for shift in 0..n {
        let position = offset + round as usize + shift;
        let idx = position % n;
        let first = offset + (idx + n - offset) % n;
        let visit = (position - first) / n;
        let Some((stratum, charts)) = pool.strata.get(idx) else {
            continue;
        };
        for attempt in 0..charts.len().min(params.max_chart_tries) {
            if let Some(&md5) = charts.get((visit + attempt) % charts.len()) {
                out.push((md5, *stratum));
            }
        }
    }
    out
}

/// One of `starts`, chosen by seed, round and chart.
pub fn pick_start(seed: u64, round: u32, md5: ChartMd5, starts: &[TimeUs]) -> Option<TimeUs> {
    if starts.is_empty() {
        return None;
    }
    let at = mix(seed, b"start", &[&round.to_le_bytes(), &md5.0]) % starts.len() as u64;
    starts.get(usize::try_from(at).unwrap_or(0)).copied()
}

/// [`plan`], then the first chart with a free window. `rows_of` gives a chart's ascending row
/// times, or `None` when it has none to offer; it is async so the service can read rows through
/// the library feature (D12) inside this same loop.
pub async fn sample<E>(
    seed: u64,
    round: u32,
    pool: &Pool,
    exclude: &[SegmentAnchor],
    window: TimeUs,
    params: &SamplerParams,
    mut rows_of: impl AsyncFnMut(ChartMd5) -> Result<Option<Vec<TimeUs>>, E>,
) -> Result<Option<Pick>, E> {
    for (md5, stratum) in plan(seed, round, pool, params) {
        let Some(rows) = rows_of(md5).await? else {
            continue;
        };
        let starts = window_starts(md5, &rows, window, exclude, params.min_rows);
        if let Some(t0) = pick_start(seed, round, md5, &starts) {
            return Ok(Some(Pick {
                md5,
                t0,
                t1: TimeUs(t0.0 + window.0),
                stratum,
            }));
        }
    }
    Ok(None)
}

/// Row times `t` whose window `[t, t + window)` lies in the chart span
/// ([`window::chart_span`]), holds at least `min_rows` rows and overlaps no anchor of this
/// chart.
pub fn window_starts(
    md5: ChartMd5,
    rows: &[TimeUs],
    window: TimeUs,
    exclude: &[SegmentAnchor],
    min_rows: usize,
) -> Vec<TimeUs> {
    let Some(span) = window::chart_span(rows) else {
        return Vec::new();
    };
    let blocked: Vec<&SegmentAnchor> = exclude.iter().filter(|a| a.chart_md5() == md5).collect();
    let mut out = Vec::new();
    for (i, &t) in rows.iter().enumerate() {
        let end = TimeUs(t.0 + window.0);
        if !window::within(t, end, span) {
            break;
        }
        let inside = rows.partition_point(|r| *r < end) - i;
        let free = !blocked.iter().any(|a| a.t0_us() < end && t < a.t1_us());
        if inside >= min_rows && free {
            out.push(t);
        }
    }
    out
}

/// Deterministic choice bits: blake3 over a domain tag, the seed and length-prefixed parts.
fn mix(seed: u64, tag: &[u8], parts: &[&[u8]]) -> u64 {
    let mut h = blake3::Hasher::new();
    h.update(b"wolluf.label.sampler.v1");
    h.update(tag);
    h.update(&seed.to_le_bytes());
    for p in parts {
        h.update(&(p.len() as u32).to_le_bytes());
        h.update(p);
    }
    let out = h.finalize();
    let b = out.as_bytes();
    u64::from_le_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]])
}

#[cfg(test)]
mod tests;
