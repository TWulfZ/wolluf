use std::collections::{BTreeMap, BTreeSet};
use std::convert::Infallible;

use wolluf_core::{ChartMd5, ColMask, Keymode, SegmentAnchor, TimeUs};

use super::*;

fn md5(n: u8) -> ChartMd5 {
    ChartMd5([n; 16])
}

fn dan(ord: f64) -> LevelLabel {
    LevelLabel {
        source: "jinjin_dan_regular".into(),
        scale: "jinjin_dan_regular".into(),
        level_ord: Some(ord),
        level_text: format!("{ord}th"),
    }
}

fn bms(scale: &str, ord: f64) -> LevelLabel {
    LevelLabel {
        source: "bms_5ynt3ck".into(),
        scale: scale.into(),
        level_ord: Some(ord),
        level_text: format!("{ord}"),
    }
}

fn facts(n: u8, nps: f64, played: bool, labels: Vec<LevelLabel>) -> ChartFacts {
    ChartFacts {
        md5: md5(n),
        nps,
        played,
        labels,
    }
}

fn ms(v: i32) -> TimeUs {
    TimeUs::from_ms(v)
}

/// A row every 250 ms over 20 s.
fn rows() -> Vec<TimeUs> {
    (0..80).map(|i| ms(i * 250)).collect()
}

fn anchor(n: u8, t0: i32, t1: i32) -> SegmentAnchor {
    SegmentAnchor::new(
        md5(n),
        ms(t0),
        ms(t1),
        ColMask::full(Keymode::K7),
        Keymode::K7,
    )
    .unwrap()
}

fn params() -> SamplerParams {
    SamplerParams::default()
}

fn pool(seed: u64, charts: &[ChartFacts], filter: &LevelFilter) -> Pool {
    Pool::new(seed, charts, &assign(charts, &params()), filter)
}

fn block<T>(f: impl std::future::Future<Output = T>) -> T {
    tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap()
        .block_on(f)
}

fn run(seed: u64, round: u32, pool: &Pool, exclude: &[SegmentAnchor]) -> Option<Pick> {
    block(sample(
        seed,
        round,
        pool,
        exclude,
        ms(4_000),
        &params(),
        async |_| Ok::<_, Infallible>(Some(rows())),
    ))
    .unwrap()
}

fn library() -> Vec<ChartFacts> {
    vec![
        facts(1, 12.0, false, vec![dan(3.0)]),
        facts(2, 12.0, false, vec![dan(3.4)]),
        facts(3, 12.0, false, vec![dan(7.0)]),
        facts(4, 25.0, false, vec![dan(7.0)]),
        facts(5, 5.0, false, vec![bms("bms_st", 1.0)]),
        facts(6, 5.0, false, vec![]),
    ]
}

#[test]
fn assign_places_charts_by_level_and_density() {
    let charts = vec![
        facts(1, 12.0, false, vec![dan(3.4)]),
        facts(2, 25.0, false, vec![bms("bms_st", 1.0), dan(9.0)]),
        facts(3, 5.0, false, vec![bms("bms_st", 1.0)]),
        facts(4, 5.0, false, vec![bms("bms_st", 2.0)]),
        facts(5, 5.0, false, vec![bms("bms_st", 3.0)]),
        facts(6, 5.0, false, vec![bms("bms_st", 4.0)]),
        facts(7, 9.99, false, vec![]),
        facts(
            8,
            f64::NAN,
            false,
            vec![LevelLabel {
                level_ord: None,
                ..dan(0.0)
            }],
        ),
    ];
    let got: BTreeMap<u8, (String, Option<String>)> = assign(&charts, &params())
        .into_iter()
        .map(|(md5, a)| (md5.0[0], (a.stratum.to_string(), a.label)))
        .collect();
    let want = |s: &str, l: Option<&str>| (s.to_owned(), l.map(str::to_owned));
    assert_eq!(
        got[&1],
        want("dan_03/nps_1", Some("jinjin_dan_regular:3.4th"))
    );
    assert_eq!(
        got[&2],
        want("dan_09/nps_3", Some("jinjin_dan_regular:9th")),
        "a dan label outranks a BMS one"
    );
    // Per-scale quartiles over the BMS levels 1, 1, 2, 3, 4 (chart 2's level 1 counts too).
    assert_eq!(got[&3], want("bms_low/nps_0", Some("bms_st:1")));
    assert_eq!(got[&4], want("bms_mid/nps_0", Some("bms_st:2")));
    assert_eq!(got[&5], want("bms_high/nps_0", Some("bms_st:3")));
    assert_eq!(got[&6], want("bms_very_high/nps_0", Some("bms_st:4")));
    assert_eq!(got[&7], want("unlevelled/nps_0", None));
    assert_eq!(
        got[&8],
        want("unlevelled/nps_0", Some("jinjin_dan_regular:0th")),
        "a label without a level still names the chart"
    );
}

#[test]
fn sampler_is_deterministic_per_seed() {
    let charts = library();
    let seq = |seed| -> Vec<Option<Pick>> {
        let pool = pool(seed, &charts, &LevelFilter::default());
        (0..12).map(|r| run(seed, r, &pool, &[])).collect()
    };
    assert_eq!(seq(7), seq(7));
    assert_ne!(seq(7), seq(8));
    assert!(seq(7).iter().all(Option::is_some));
}

#[test]
fn sampler_round_robins_across_strata() {
    let charts = library();
    let pool = pool(3, &charts, &LevelFilter::default());
    let assigned = assign(&charts, &params());
    let strata: BTreeSet<Stratum> = assigned.values().map(|a| a.stratum).collect();
    assert_eq!(pool.len(), strata.len());
    let picks: Vec<Pick> = (0..strata.len() as u32)
        .map(|r| run(3, r, &pool, &[]).unwrap())
        .collect();
    let seen: BTreeSet<Stratum> = picks.iter().map(|p| p.stratum).collect();
    assert_eq!(
        seen, strata,
        "one pass visits every stratum once: {picks:?}"
    );
    for pick in &picks {
        assert_eq!(assigned[&pick.md5].stratum, pick.stratum);
    }
}

#[test]
fn sampler_starts_on_a_row_and_never_overlaps_labelled_anchors() {
    let charts = vec![facts(1, 12.0, false, vec![dan(3.0)])];
    let pool = pool(11, &charts, &LevelFilter::default());
    let mut labelled = vec![anchor(1, 2_000, 6_000), anchor(1, 9_000, 13_000)];
    let rows = rows();
    for round in 0..40 {
        let Some(pick) = run(11, round, &pool, &labelled) else {
            break;
        };
        assert!(rows.contains(&pick.t0), "{pick:?}");
        assert_eq!(pick.t1, TimeUs(pick.t0.0 + 4_000_000));
        let new = anchor(
            1,
            pick.t0.as_ms_floor() as i32,
            pick.t1.as_ms_floor() as i32,
        );
        assert!(!labelled.iter().any(|a| a.overlaps(&new)), "{pick:?}");
        labelled.push(new);
    }
    assert_eq!(
        run(11, 99, &pool, &labelled),
        None,
        "a chart covered by labels offers nothing"
    );
}

#[test]
fn window_needs_enough_rows_and_must_fit_the_chart() {
    let sparse = [ms(0), ms(3_000), ms(3_100), ms(3_200), ms(3_300), ms(9_000)];
    let starts = window_starts(md5(1), &sparse, ms(1_000), &[], 4);
    assert_eq!(starts, [ms(3_000)]);
    let short = [ms(0), ms(100), ms(200), ms(300)];
    assert!(window_starts(md5(1), &short, ms(4_000), &[], 1).is_empty());
    let other_chart = [anchor(2, 0, 10_000)];
    assert_eq!(
        window_starts(md5(1), &sparse, ms(1_000), &other_chart, 4),
        [ms(3_000)],
        "anchors of other charts do not block"
    );
}

#[test]
fn sampler_prefers_played_charts() {
    let charts: Vec<ChartFacts> = (1..=6)
        .map(|n| facts(n, 12.0, n == 4, vec![dan(5.0)]))
        .collect();
    for seed in 0..10 {
        let pool = pool(seed, &charts, &LevelFilter::default());
        assert_eq!(run(seed, 0, &pool, &[]).unwrap().md5, md5(4), "seed {seed}");
        let second = run(seed, 1, &pool, &[]).unwrap().md5;
        assert_ne!(second, md5(4), "the next visit moves on, seed {seed}");
    }
}

#[test]
fn sampler_skips_charts_without_rows() {
    let charts = vec![
        facts(1, 12.0, false, vec![dan(3.0)]),
        facts(2, 12.0, false, vec![dan(3.0)]),
    ];
    let pool = pool(5, &charts, &LevelFilter::default());
    for round in 0..4 {
        let pick = block(sample(
            5,
            round,
            &pool,
            &[],
            ms(4_000),
            &params(),
            async |m| Ok::<_, Infallible>((m == md5(2)).then(rows)),
        ))
        .unwrap()
        .unwrap();
        assert_eq!(pick.md5, md5(2));
    }
}

#[test]
fn filter_keeps_charts_with_a_matching_label() {
    let charts = library();
    let md5s = |filter: LevelFilter| -> BTreeSet<u8> {
        let pool = pool(1, &charts, &filter);
        pool.charts().map(|m| m.0[0]).collect()
    };
    assert_eq!(md5s(LevelFilter::default()).len(), 6);
    let by_scale = LevelFilter {
        scale: Some("jinjin_dan_regular".into()),
        ..LevelFilter::default()
    };
    assert_eq!(md5s(by_scale.clone()), [1, 2, 3, 4].into());
    assert_eq!(
        md5s(LevelFilter {
            level_min: Some(3.2),
            level_max: Some(7.0),
            ..by_scale
        }),
        [2, 3, 4].into()
    );
    assert_eq!(
        md5s(LevelFilter {
            level_max: Some(1.0),
            ..LevelFilter::default()
        }),
        [5].into(),
        "a bound alone matches any scale"
    );
}

#[test]
fn empty_pool_samples_nothing() {
    let pool = pool(1, &[], &LevelFilter::default());
    assert!(pool.is_empty());
    assert_eq!(run(1, 0, &pool, &[]), None);
}
