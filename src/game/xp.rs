//! Review XP and levels.

use super::*;

pub fn level_need(level: u64) -> u64 {
    ((level as f64).powf(1.5) * 10.0).round() as u64 * 10
}

pub fn level_for(xp: u64) -> (u64, u64, u64) {
    let mut level = 1;
    let mut rest = xp;
    loop {
        let need = level_need(level);
        if rest < need {
            return (level, rest, need);
        }
        rest -= need;
        level += 1;
    }
}

/// How review XP is weighted, chosen once per server with `review_weighting`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewWeighting {
    /// Every answer earns its full type rate; learning and relearning steps are 6 XP.
    #[default]
    Flat,
    /// Learning steps are 9 XP, and answering the same card again on the same day
    /// is worth half as much each time.
    Diminishing,
}

static DIMINISHING: AtomicBool = AtomicBool::new(false);

/// Sets the server's review weighting. XP is computed from history on demand,
/// so every profile follows a change from the next request on.
pub fn set_review_weighting(weighting: ReviewWeighting) {
    DIMINISHING.store(weighting == ReviewWeighting::Diminishing, Ordering::Relaxed);
}

pub fn review_weighting() -> ReviewWeighting {
    if DIMINISHING.load(Ordering::Relaxed) {
        ReviewWeighting::Diminishing
    } else {
        ReviewWeighting::Flat
    }
}

#[cfg(test)]
pub(super) fn review_base_xp(r: &Review) -> f64 {
    review_base_xp_with(r, ReviewWeighting::Flat)
}

pub(super) fn review_base_xp_with(r: &Review, weighting: ReviewWeighting) -> f64 {
    let base = match (r.kind, weighting) {
        (1, _) => 10.0,
        (0, ReviewWeighting::Diminishing) => 9.0,
        (0 | 2, _) => 6.0,
        _ => 2.0,
    };
    if r.last_ivl >= MATURE_IVL {
        base + 5.0
    } else {
        base
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::fixtures::*;

    #[test]
    fn levels_are_monotonic() {
        assert_eq!(level_for(0), (1, 0, 100));
        assert_eq!(level_for(99).0, 1);
        assert_eq!(level_for(100), (2, 0, 280));
        let mut last = 0;
        for level in 1..200 {
            assert!(level_need(level) > last);
            last = level_need(level);
        }
    }

    #[test]
    fn answer_button_never_matters() {
        let r = Review {
            id: 0,
            cid: 1,
            last_ivl: 30,
            ivl: None,
            time_ms: 4_000,
            kind: 1,
        };
        assert_eq!(review_base_xp(&r), 15.0);
    }

    #[test]
    fn quick_answers_earn_full_xp() {
        let r = Review {
            id: 0,
            cid: 1,
            last_ivl: 30,
            ivl: None,
            time_ms: 200,
            kind: 1,
        };
        assert_eq!(review_base_xp(&r), 15.0);
    }

    #[test]
    fn repeating_a_card_keeps_the_same_xp_as_answering_different_cards() {
        let answer = |day: i64, index: i64, cid: i64| Review {
            id: day * DAY_MS + NOON + index * 10_000,
            cid,
            last_ivl: 0,
            ivl: None,
            time_ms: 5_000,
            kind: 1,
        };
        let same_card: Vec<Review> = (0..6).map(|i| answer(1, i, 7)).collect();
        let (_, _, repeated) = collect_days(&same_card, &utc());
        let varied: Vec<Review> = (0..6).map(|i| answer(1, i, 7 + i)).collect();
        let (_, _, spread) = collect_days(&varied, &utc());
        assert_eq!(repeated, spread, "card identity does not reduce review XP");
        assert!(repeated[5].1 > repeated[0].1, "the combo bonus still grows");

        let (_, _, fresh) = collect_days(&[answer(1, 0, 7), answer(2, 0, 7)], &utc());
        assert_eq!(fresh[0].1, fresh[1].1, "a new day resets the combo");
    }

    #[test]
    fn learning_and_relearning_keep_the_original_rate_for_every_step() {
        for kind in [0, 2] {
            let steps: Vec<Review> = (0..3)
                .map(|index| Review {
                    id: DAY_MS + NOON + index * 600_000,
                    cid: 7,
                    last_ivl: 0,
                    ivl: None,
                    time_ms: 5_000,
                    kind,
                })
                .collect();
            let (_, _, earned) = collect_days(&steps, &utc());
            for (_, xp) in earned {
                assert!((xp - 6.03).abs() < 1e-9, "kind {kind} earned {xp}");
            }
        }
    }

    #[test]
    fn diminishing_weighting_pays_less_for_each_repeat_of_a_card() {
        let answer = |day: i64, index: i64, cid: i64| Review {
            id: day * DAY_MS + NOON + index * 10_000,
            cid,
            last_ivl: 0,
            ivl: None,
            time_ms: 5_000,
            kind: 1,
        };
        let run = |reviews: &[Review]| {
            collect_days_weighted(
                exact(reviews),
                &ClockHistory::single(utc()),
                0,
                ReviewWeighting::Diminishing,
            )
            .2
        };
        let total = |earned: &[(i64, f64)]| earned.iter().map(|(_, xp)| xp).sum::<f64>();

        let stuck: Vec<Review> = (0..6).map(|i| answer(1, i, 7)).collect();
        let repeated = run(&stuck);
        for index in 1..repeated.len() {
            assert!(
                repeated[index].1 < repeated[index - 1].1,
                "answer {index} should pay less than the one before it"
            );
        }
        assert!(
            total(&repeated) < 3.0 * repeated[0].1,
            "one card cannot be farmed"
        );

        let varied: Vec<Review> = (0..6).map(|i| answer(1, i, 7 + i)).collect();
        assert!(
            total(&run(&varied)) > 2.0 * total(&repeated),
            "studying six cards must beat answering one card six times"
        );

        let fresh = run(&[answer(1, 0, 7), answer(2, 0, 7)]);
        assert_eq!(fresh[0].1, fresh[1].1, "a new day starts the card over");
    }

    #[test]
    fn diminishing_weighting_pays_learning_about_as_much_as_a_review() {
        let answer = |index: i64, kind: u8, last_ivl: i64| Review {
            id: DAY_MS + NOON + index * 600_000,
            cid: 7,
            last_ivl,
            ivl: None,
            time_ms: 5_000,
            kind,
        };
        let run = |reviews: &[Review]| {
            collect_days_weighted(
                exact(reviews),
                &ClockHistory::single(utc()),
                0,
                ReviewWeighting::Diminishing,
            )
            .2
        };
        let learning = run(&[answer(0, 0, 0), answer(1, 0, 0), answer(2, 0, 0)]);
        let learned: f64 = learning.iter().map(|(_, xp)| xp).sum();
        let known = run(&[answer(0, 1, 30)])[0].1;
        assert!(
            (learned - known).abs() < 2.0,
            "learning {learned}, knowing {known}"
        );
        assert!(
            learning[0].1 < known,
            "a single learning step is not a whole review"
        );

        let lapse = run(&[answer(0, 1, 30), answer(1, 2, 0), answer(2, 2, 0)]);
        let relearning: f64 = lapse[1..].iter().map(|(_, xp)| xp).sum();
        assert!(
            relearning < lapse[0].1 / 2.0,
            "relearning {relearning} vs {}",
            lapse[0].1
        );
    }

    #[test]
    fn review_weighting_defaults_to_flat_in_config() {
        #[derive(Deserialize)]
        struct C {
            #[serde(default)]
            w: ReviewWeighting,
        }
        let parse = |s: &str| serde_json::from_str::<C>(s).unwrap().w;
        assert_eq!(parse("{}"), ReviewWeighting::Flat);
        assert_eq!(parse(r#"{"w":"flat"}"#), ReviewWeighting::Flat);
        assert_eq!(
            parse(r#"{"w":"diminishing"}"#),
            ReviewWeighting::Diminishing
        );
    }

    #[test]
    fn xp_sums_into_heatmap_and_total() {
        let p = compute(
            "a",
            "a",
            &history(&[1, 2, 3]),
            &utc(),
            &Week::default(),
            at(3),
        );
        let heat: u64 = p.heatmap.iter().map(|c| c.xp).sum();
        assert_eq!(heat, p.xp_total);
        assert!(p.xp_total > 0);
        assert_eq!(p.week_xp, p.xp_total);
    }
}
