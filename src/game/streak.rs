//! Streaks: the review minutes a day needs, freezes, and the verdict on each day.

use super::*;

pub const MAX_FREEZES: u32 = 3;

/// Preference changes ordered by time, retaining save order for equal timestamps.
/// Replaying them with reviews keeps later toggles from changing past protection.
#[derive(Clone, Debug, PartialEq)]
pub struct FreezePreference {
    pub at: i64,
    pub enabled: bool,
}

/// From `at` on, a day needs `minutes` of review to count toward the streak.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StreakChange {
    pub at: i64,
    pub minutes: u32,
}

/// The review minutes a day needs to keep the streak, as the server's
/// `streak_minutes` setting changed over time, oldest first. A day follows the
/// value in force when it began, so a change never rewrites a day that already
/// started. Before the first change any review counts.
///
/// `players` holds an admin's per-player overrides, oldest first for each
/// player. `None` hands the player back to the server's value. They follow the
/// same rule: a change applies from the player's next day on.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StreakRule {
    pub changes: Vec<StreakChange>,
    pub players: BTreeMap<String, Vec<PlayerStreakChange>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PlayerStreakChange {
    pub at: i64,
    pub minutes: Option<u32>,
}

impl StreakRule {
    pub fn minutes_at(&self, at: i64) -> u32 {
        let end = self.changes.partition_point(|change| change.at <= at);
        if end > 0 {
            self.changes[end - 1].minutes
        } else {
            0
        }
    }

    /// The rule one player follows: the server's changes with that player's
    /// overrides on top, as a plain list of changes.
    pub fn for_player(&self, user: &str) -> StreakRule {
        let Some(own) = self.players.get(user).filter(|own| !own.is_empty()) else {
            return StreakRule {
                changes: self.changes.clone(),
                players: BTreeMap::new(),
            };
        };
        let mut times: Vec<i64> = self
            .changes
            .iter()
            .map(|change| change.at)
            .chain(own.iter().map(|change| change.at))
            .collect();
        times.sort_unstable();
        times.dedup();
        let changes = times
            .into_iter()
            .map(|at| {
                let end = own.partition_point(|change| change.at <= at);
                let minutes = own[..end]
                    .last()
                    .and_then(|change| change.minutes)
                    .unwrap_or_else(|| self.minutes_at(at));
                StreakChange { at, minutes }
            })
            .collect();
        StreakRule {
            changes,
            players: BTreeMap::new(),
        }
    }
}

static STREAK_RULE: RwLock<StreakRule> = RwLock::new(StreakRule {
    changes: Vec::new(),
    players: BTreeMap::new(),
});

static STREAK_RULE_VERSION: AtomicU64 = AtomicU64::new(0);

/// Sets the server's streak rule. Profiles are computed on demand, so they all
/// follow it from the next request on.
pub fn set_streak_rule(rule: StreakRule) {
    let mut current = STREAK_RULE.write().unwrap_or_else(|e| e.into_inner());
    if *current != rule {
        *current = rule;
        STREAK_RULE_VERSION.fetch_add(1, Ordering::Relaxed);
    }
}

/// The rule `user` follows, with any per-player override applied.
pub fn streak_rule_for(user: &str) -> StreakRule {
    STREAK_RULE
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .for_player(user)
}

/// Changes whenever [`set_streak_rule`] changes the rule, so cached profiles know they are stale.
pub fn streak_rule_version() -> u64 {
    STREAK_RULE_VERSION.load(Ordering::Relaxed)
}

#[derive(Clone, Debug, Default)]
pub struct FreezePolicy {
    /// First migration time for an existing database; earlier Anki days retain
    /// their old automatic protection. New databases have no legacy period.
    pub legacy_until: Option<i64>,
    /// Saved rollout time: changing the default must not rewrite earlier rewards.
    /// An unrestricted default policy enables protection throughout its history.
    pub default_enabled_from: Option<i64>,
    pub preferences: Vec<FreezePreference>,
    /// Days an admin protected by hand, as `YYYY-MM-DD` on the player's clock.
    /// They keep the streak like a spent freeze but leave the bank alone.
    pub granted: BTreeSet<String>,
}

impl FreezePolicy {
    pub fn enabled_at(&self, at: i64) -> bool {
        let end = self.preferences.partition_point(|change| change.at <= at);
        if end > 0 {
            self.preferences[end - 1].enabled
        } else {
            self.default_enabled_from.is_none_or(|start| at >= start)
        }
    }
}

/// The server's verdict on one Anki day. Clients colour and word days by it instead of
/// judging the reviews themselves, so every screen says the same of a day.
#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DayState {
    /// The day reached the streak minimum and counts toward the streak.
    Met,
    /// Reviews short of the minimum. Today stays short until it is met or ends.
    Short,
    /// A freeze or an admin's grant kept the streak through the day, reviewed or not.
    Frozen,
    /// Moving the clock east stepped over the day, so nobody could study in it. It keeps
    /// the streak like a frozen day.
    Skipped,
    /// No reviews, and nothing kept the streak.
    None,
}

impl DayState {
    /// The verdict on a day with `reviews` taking `time_ms`, which needed `needed_ms` to
    /// count, and was `frozen` (or `skipped`) after it ended. A covered day reads as
    /// covered even with reviews: it did not count on its own.
    pub fn of(reviews: u64, time_ms: i64, needed_ms: i64, frozen: bool, skipped: bool) -> Self {
        if reviews > 0 && time_ms >= needed_ms {
            DayState::Met
        } else if skipped && frozen {
            DayState::Skipped
        } else if frozen {
            DayState::Frozen
        } else if reviews > 0 {
            DayState::Short
        } else {
            DayState::None
        }
    }
}

/// Streak lengths worth a message of their own, beyond any streak achievement.
pub(super) fn streak_milestone(streak: u64) -> bool {
    matches!(streak, 7 | 30 | 50 | 100 | 200 | 365 | 500)
        || (streak > 365 && streak.is_multiple_of(365))
}

#[derive(Serialize, Clone, Copy, Debug)]
#[serde(rename_all = "snake_case")]
pub enum StreakState {
    Studied,
    Pending,
    Protected,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::fixtures::*;

    #[test]
    fn streak_status_separates_today_reviews_from_previous_day_protection() {
        let reviews = perfect_day(&[], 1, &utc());
        let changes = [protection(0, true)];
        let studied = protected(&reviews, at(1), &changes);
        let pending = protected(&reviews, at(2), &changes);
        let frozen = protected(&reviews, at(3), &changes);
        for (profile, expected) in [
            (&studied, "studied"),
            (&pending, "pending"),
            (&frozen, "protected"),
        ] {
            assert_eq!(
                serde_json::to_value(profile).unwrap()["streak_state"],
                expected
            );
        }
        assert_eq!(frozen.today.reviews, 0);
        assert_eq!(frozen.streak, pending.streak);
        let mut resumed = reviews;
        resumed.extend(reviews_on(3, 1, 30_000));
        let resumed = protected(&resumed, at(3), &changes);
        assert_eq!(
            serde_json::to_value(resumed).unwrap()["streak_state"],
            "studied"
        );
    }

    #[test]
    fn streak_status_changes_at_the_players_anki_cutoff() {
        let reviews = reviews_on(1, 1, 1000);
        let before = protected(&reviews, at(1), &[]);
        let after = protected(&reviews, before.day_ends_at, &[]);
        assert_eq!(
            serde_json::to_value(&before).unwrap()["streak_state"],
            "studied"
        );
        assert_eq!(
            serde_json::to_value(&after).unwrap()["streak_state"],
            "pending"
        );
        assert_eq!(after.streak, before.streak);
    }

    #[test]
    fn streak_counts_consecutive_days() {
        let p = compute(
            "a",
            "a",
            &history(&[10, 11, 12]),
            &utc(),
            &Week::default(),
            at(12),
        );
        assert_eq!(p.streak, 3);
        assert!(!p.at_risk);
    }

    #[test]
    fn today_without_reviews_is_at_risk_not_broken() {
        let p = compute(
            "a",
            "a",
            &history(&[10, 11, 12]),
            &utc(),
            &Week::default(),
            at(13),
        );
        assert_eq!(p.streak, 3);
        assert!(p.at_risk);
    }

    #[test]
    fn granted_days_keep_the_streak_without_spending_the_bank() {
        let reviews = history(&[10, 14]);
        let policy = FreezePolicy {
            granted: [11, 12, 13].map(date_string).into(),
            ..FreezePolicy::default()
        };
        let p = compute_with_freezes(
            "a",
            "a",
            &reviews,
            &utc(),
            &Week::default(),
            at(14),
            &policy,
        );
        assert_eq!(p.streak, 2);
        assert_eq!(p.stored_freezes, 0);
        let frozen: Vec<_> = p
            .heatmap
            .iter()
            .filter(|c| c.frozen)
            .map(|c| c.date.clone())
            .collect();
        assert_eq!(frozen, [11, 12, 13].map(date_string));
        let partial = FreezePolicy {
            granted: [11, 12].map(date_string).into(),
            ..FreezePolicy::default()
        };
        let broken = compute_with_freezes(
            "a",
            "a",
            &reviews,
            &utc(),
            &Week::default(),
            at(14),
            &partial,
        );
        assert_eq!(broken.streak, 1);
    }

    #[test]
    fn missed_day_without_freeze_resets() {
        let p = compute(
            "a",
            "a",
            &history(&[10, 11, 13]),
            &utc(),
            &Week::default(),
            at(13),
        );
        assert_eq!(p.streak, 1);
        assert_eq!(p.lifetime.best_streak, 2);
    }

    #[test]
    fn freezes_default_on_and_earn_from_quests_without_a_settings_change() {
        let empty = protected(&[], at(1), &[]);
        assert!(empty.freezes_enabled);
        assert_eq!(empty.stored_freezes, 0);
        let reviews = perfect_history(&[1]);
        let earned = protected(&reviews, at(1), &[]);
        assert!(earned.freezes_enabled);
        assert_eq!(earned.freezes, 1);
        assert!(earned.freeze_earned_today);
        let paused = protected(&reviews, at(1), &[protection(at(1), false)]);
        assert!(!paused.freezes_enabled);
        assert_eq!(paused.stored_freezes, 1);
        assert_eq!(paused.freezes, 0);
        let covered = protected(&reviews, at(3), &[]);
        assert_eq!(covered.streak, 1);
        assert_eq!(covered.heatmap.iter().filter(|cell| cell.frozen).count(), 1);
    }

    #[test]
    fn freeze_default_rollout_does_not_backfill_and_keeps_explicit_opt_outs() {
        let mut reviews = perfect_history(&[1, 2]);
        let policy = FreezePolicy {
            default_enabled_from: Some(utc().day_start_ms(3)),
            ..FreezePolicy::default()
        };
        let run = |reviews: &[Review], now, policy: &FreezePolicy| {
            compute_with_freezes("a", "a", reviews, &utc(), &Week::default(), now, policy)
        };
        let boundary = policy.default_enabled_from.unwrap();
        let old = run(&reviews, boundary - 1, &policy);
        let enabled = run(&reviews, boundary, &policy);
        assert!(!old.freezes_enabled);
        assert!(enabled.freezes_enabled);
        assert_eq!(enabled.stored_freezes, 0);
        assert_eq!(enabled.xp_total, old.xp_total);
        assert_eq!(enabled.streak, old.streak);
        let earlier = run(&reviews, at(2), &policy);
        let old_rule = run(
            &reviews,
            at(2),
            &FreezePolicy {
                preferences: vec![protection(0, false)],
                ..FreezePolicy::default()
            },
        );
        assert_eq!(
            serde_json::to_value(earlier).unwrap(),
            serde_json::to_value(old_rule).unwrap()
        );
        reviews.extend(perfect_day(&reviews, 3, &utc()));
        let earned = run(&reviews, at(4), &policy);
        assert_eq!(earned.stored_freezes, 1);
        let off = FreezePolicy {
            preferences: vec![protection(at(1), false)],
            ..policy.clone()
        };
        assert!(!run(&reviews, at(4), &off).freezes_enabled);
        assert_eq!(run(&reviews, at(4), &off).stored_freezes, 0);
        let opted_in = FreezePolicy {
            preferences: vec![protection(0, true)],
            ..policy
        };
        assert_eq!(run(&reviews, at(4), &opted_in).stored_freezes, 3);
    }

    #[test]
    fn explicitly_disabled_freezes_start_empty_without_historical_rewards() {
        let reviews = perfect_history(&[1, 2, 3, 4, 5, 6, 7]);
        let p = protected(&reviews, at(9), &[protection(0, false)]);
        assert!(!p.freezes_enabled);
        assert_eq!(p.stored_freezes, 0);
        assert_eq!(p.freezes, 0);
        assert_eq!(p.streak, 0);
        assert!(p.heatmap.iter().all(|c| !c.frozen));
        let opted_in = protected(
            &reviews,
            at(9),
            &[protection(0, false), protection(at(9), true)],
        );
        assert!(opted_in.freezes_enabled);
        assert_eq!(opted_in.freezes, 0);
        assert!(!opted_in.freeze_earned_today);
        assert_eq!(protected(&[], at(9), &[protection(0, true)]).freezes, 0);
        let ordinary = protected(
            &history(&[1, 2, 3, 4, 5, 6, 7]),
            at(7),
            &[protection(0, true)],
        );
        assert_eq!(
            ordinary.freezes, 0,
            "a seven-day streak alone earns nothing"
        );
    }

    #[test]
    fn migration_preserves_previously_protected_days_and_streak() {
        let reviews = history(&[1, 2, 3, 4, 5, 6, 7, 9]);
        let policy = FreezePolicy {
            legacy_until: Some(at(10)),
            preferences: vec![],
            default_enabled_from: Some(at(10)),
            ..FreezePolicy::default()
        };
        let p = compute_with_freezes(
            "a",
            "a",
            &reviews,
            &utc(),
            &Week::default(),
            at(10),
            &policy,
        );
        assert_eq!(
            p.streak, 8,
            "upgrading must not erase the freeze that covered day8"
        );
        assert_eq!(p.lifetime.best_streak, 8);
        assert!(
            p.heatmap
                .iter()
                .any(|c| c.date == date_string(8) && c.frozen)
        );
        assert_eq!(p.freezes, 0);
        assert_eq!(p.stored_freezes, 0);
        assert!(p.freezes_enabled);
        let before_cutover = compute_with_freezes(
            "a",
            "a",
            &reviews,
            &utc(),
            &Week::default(),
            at(9),
            &FreezePolicy {
                legacy_until: Some(at(10)),
                preferences: vec![],
                default_enabled_from: Some(at(10)),
                ..FreezePolicy::default()
            },
        );
        assert_eq!(p.xp_total, before_cutover.xp_total);
        assert_eq!(p.lifetime.quests, before_cutover.lifetime.quests);
        for cell in before_cutover
            .heatmap
            .iter()
            .filter(|c| c.reviews > 0 || c.frozen)
        {
            let after = p.heatmap.iter().find(|c| c.date == cell.date).unwrap();
            assert_eq!(
                (after.reviews, after.xp, after.frozen),
                (cell.reviews, cell.xp, cell.frozen)
            );
        }
    }

    #[test]
    fn migration_clears_unspent_legacy_stock_and_new_quests_use_the_new_rules() {
        let mut reviews = history(&(1..=14).collect::<Vec<_>>());
        let policy = FreezePolicy {
            legacy_until: Some(utc().day_start_ms(15)),
            preferences: vec![protection(utc().day_start_ms(15), true)],
            ..FreezePolicy::default()
        };
        let run = |reviews: &[Review], day| {
            compute_with_freezes(
                "a",
                "a",
                reviews,
                &utc(),
                &Week::default(),
                at(day),
                &policy,
            )
        };
        let old = run(&reviews, 14);
        assert_eq!(old.stored_freezes, 2);
        let upgraded = run(&reviews, 15);
        assert_eq!(upgraded.stored_freezes, 0);
        assert_eq!(upgraded.streak, 14);
        assert_eq!(upgraded.xp_total, old.xp_total);
        assert_eq!(
            run(&reviews, 16).streak,
            0,
            "old stock cannot cover a new missed day"
        );
        reviews.extend(perfect_day(&reviews, 15, &utc()));
        let completed = run(&reviews, 15);
        assert_eq!(completed.freezes, 1);
        assert!(completed.freeze_earned_today);
        let covered = run(&reviews, 17);
        assert_eq!(covered.freezes, 0);
        assert_eq!(covered.streak, 15);
    }

    #[test]
    fn freeze_requires_all_quests_and_is_earned_only_once_per_day() {
        let mut reviews = perfect_history(&[1]);
        let completion = first_completion(&reviews, 1);
        let changes = [protection(0, true)];
        let before: Vec<_> = reviews
            .iter()
            .copied()
            .filter(|r| r.id < completion)
            .collect();
        let p = protected(&before, completion - 1, &changes);
        assert!(p.quests.iter().any(|q| !q.done));
        assert_eq!(p.freezes, 0);

        let p = protected(&reviews, at(1), &changes);
        assert!(p.quests.iter().all(|q| q.done));
        assert!(p.freeze_earned_today);
        assert_eq!(p.freezes, 1);
        assert_eq!(
            serde_json::to_value(&p).unwrap(),
            serde_json::to_value(protected(&reviews, at(1), &changes)).unwrap(),
            "repeated reads replay the same result"
        );
        reviews.extend(reviews_on(1, 5, 99));
        assert_eq!(protected(&reviews, at(1), &changes).freezes, 1);
    }

    #[test]
    fn freeze_reward_matches_visible_quest_completion_after_day_rollover() {
        let mut reviews = perfect_history(&[1]);
        let first_today = reviews.len();
        reviews.extend(perfect_day(&reviews, 2, &utc()));
        // Read the visible quest state independently of the collector's saved
        // completion timestamp, including the previous day's adaptive targets.
        let completed = (first_today..reviews.len())
            .find(|&i| {
                protected(&reviews[..=i], reviews[i].id, &[])
                    .quests
                    .iter()
                    .all(|quest| quest.done)
            })
            .expect("the second day completes its displayed quests");
        let completion = reviews[completed].id;
        assert!(
            protected(&reviews[..completed], completion - 1, &[])
                .quests
                .iter()
                .any(|quest| !quest.done)
        );
        assert_eq!(
            protected(
                &reviews,
                at(2),
                &[protection(0, false), protection(completion, true)]
            )
            .stored_freezes,
            1,
            "opting in at the visible completion earns today's freeze"
        );
        assert_eq!(
            protected(
                &reviews,
                at(2),
                &[protection(0, false), protection(completion + 1, true)]
            )
            .stored_freezes,
            0,
            "opting in after completion cannot claim a past reward"
        );
    }

    #[test]
    fn preference_at_first_completion_controls_earning_without_backfill() {
        let mut reviews = perfect_history(&[1]);
        let completion = first_completion(&reviews, 1);
        assert_eq!(
            protected(
                &reviews,
                at(1),
                &[protection(0, false), protection(completion, true)]
            )
            .freezes,
            1
        );
        assert_eq!(
            protected(
                &reviews,
                at(1),
                &[protection(0, false), protection(completion + 1, true)]
            )
            .freezes,
            0
        );
        reviews.extend(reviews_on(1, 3, 999));
        let p = protected(
            &reviews,
            at(1),
            &[
                protection(0, true),
                protection(completion, false),
                protection(completion + 1, true),
            ],
        );
        assert_eq!(
            p.freezes, 0,
            "more reviews cannot retry a missed earning opportunity"
        );
        let p = protected(
            &reviews,
            at(1),
            &[protection(0, true), protection(completion + 1, false)],
        );
        assert_eq!(
            p.stored_freezes, 1,
            "turning off keeps what was already earned"
        );
        assert_eq!(
            p.freezes, 0,
            "older clients must not promise disabled coverage"
        );
    }

    #[test]
    fn freezes_cap_at_three_and_can_be_earned_again_after_spending() {
        let reviews = perfect_history(&[1, 2, 3, 4]);
        let changes = [protection(0, true)];
        let full = protected(&reviews, at(4), &changes);
        assert_eq!(MAX_FREEZES, 3);
        assert_eq!(full.freezes, MAX_FREEZES);
        assert!(
            !full.freeze_earned_today,
            "completing quests at capacity grants no extra charge"
        );
        let mut after_gap = reviews;
        after_gap.extend(perfect_day(&after_gap, 6, &utc()));
        let p = protected(&after_gap, at(6), &changes);
        assert_eq!(p.freezes, 3);
        assert!(p.freeze_earned_today);
        assert_eq!(
            p.streak, 5,
            "covered days preserve rather than increase the streak"
        );
        let frozen = p.heatmap.iter().find(|c| c.date == date_string(5)).unwrap();
        assert!(frozen.frozen);
        assert_eq!(frozen.reviews, 0);
        assert_eq!(frozen.xp, achievement_xp(Metric::FreezesUsed, 1));
    }

    #[test]
    fn ended_missed_days_spend_one_each_until_empty() {
        let reviews = perfect_history(&[1, 2, 3]);
        let changes = [protection(0, true)];
        let today = protected(&reviews, at(4), &changes);
        assert_eq!(today.freezes, 3, "today has not ended yet");
        assert_eq!(today.streak, 3);
        let p = protected(&reviews, at(7), &changes);
        assert_eq!(p.freezes, 0);
        assert_eq!(p.streak, 3);
        assert_eq!(p.heatmap.iter().filter(|c| c.frozen).count(), 3);
        let broken = protected(&reviews, at(8), &changes);
        assert_eq!(broken.streak, 0);
        assert_eq!(broken.heatmap.iter().filter(|c| c.frozen).count(), 3);
    }

    #[test]
    fn disabling_preserves_bank_and_past_coverage_but_does_not_cover_new_gaps() {
        let reviews = perfect_history(&[1, 2, 3]);
        let off = [protection(0, true), protection(at(5), false)];
        let paused = protected(&reviews, at(6), &off);
        assert_eq!(paused.stored_freezes, 2);
        assert_eq!(paused.freezes, 0);
        assert_eq!(paused.streak, 0);
        assert_eq!(paused.heatmap.iter().filter(|c| c.frozen).count(), 1);
        assert!(
            paused
                .heatmap
                .iter()
                .any(|c| c.date == date_string(4) && c.frozen)
        );
        let mut on_again = off.to_vec();
        on_again.push(protection(at(6), true));
        let p = protected(&reviews, at(6), &on_again);
        assert_eq!(p.freezes, 2);
        assert_eq!(
            p.streak, 0,
            "reenabling cannot retroactively cover yesterday"
        );
        assert_eq!(p.heatmap.iter().filter(|c| c.frozen).count(), 1);
        assert_eq!(
            protected(&reviews, at(8), &on_again).freezes,
            2,
            "an already broken streak spends nothing"
        );
    }

    #[test]
    fn missed_day_checks_setting_just_before_the_anki_rollover() {
        let clock = Clock {
            offset_west_min: -120,
            rollover_hour: 4,
        };
        let reviews = perfect_day(&[], 1, &clock);
        let boundary = clock.day_start_ms(3);
        let run = |now, changes: &[FreezePreference]| {
            compute_with_freezes(
                "a",
                "a",
                &reviews,
                &clock,
                &Week::default(),
                now,
                &FreezePolicy {
                    preferences: changes.to_vec(),
                    ..FreezePolicy::default()
                },
            )
        };
        assert_eq!(run(boundary - 1, &[protection(0, true)]).stored_freezes, 1);
        let p = run(
            boundary,
            &[protection(0, true), protection(boundary, false)],
        );
        assert_eq!(
            p.stored_freezes, 0,
            "a change on the new day preserves yesterday's enabled state"
        );
        assert_eq!(p.streak, 1);
        let p = run(
            boundary,
            &[protection(0, true), protection(boundary - 1, false)],
        );
        assert_eq!(p.stored_freezes, 1);
        assert_eq!(p.streak, 0);
        let p = run(
            boundary,
            &[
                protection(0, true),
                protection(at(1), false),
                protection(boundary, true),
            ],
        );
        assert_eq!(p.streak, 0);
        assert_eq!(p.freezes, 1);
    }

    #[test]
    fn future_reviews_and_preferences_cannot_award_a_freeze_early() {
        let reviews = perfect_history(&[1]);
        let completion = first_completion(&reviews, 1);
        let changes = [protection(0, true)];
        let before = protected(&reviews, completion - 1, &changes);
        assert_eq!(before.freezes, 0);
        assert!(!before.freeze_earned_today);
        assert_eq!(protected(&reviews, completion, &changes).freezes, 1);
        let future = protected(
            &reviews,
            at(1),
            &[protection(0, false), protection(at(2), true)],
        );
        assert!(!future.freezes_enabled);
        assert_eq!(future.stored_freezes, 0);
        assert_eq!(
            changes,
            [protection(0, true)],
            "preview and replay never mutate preferences"
        );
    }

    #[test]
    fn preference_changes_at_the_same_time_use_the_latest_save() {
        let reviews = perfect_history(&[1]);
        let completion = first_completion(&reviews, 1);
        let changes = [protection(completion, true), protection(completion, false)];
        assert_eq!(protected(&reviews, at(1), &changes).stored_freezes, 0);
        let changes = [protection(completion, false), protection(completion, true)];
        assert_eq!(protected(&reviews, at(1), &changes).freezes, 1);
    }

    #[test]
    fn streak_milestones_without_an_achievement_get_their_own_message() {
        let fifty: Vec<i64> = (1..=50).collect();
        let p = compute("a", "a", &history(&fifty), &utc(), &Week::default(), at(50));
        assert_eq!(p.streak, 50);
        assert!(
            p.events
                .iter()
                .any(|e| e.key == "streak:1:50" && e.title == "50-day streak!")
        );

        let week: Vec<i64> = (1..=7).collect();
        let p = compute("a", "a", &history(&week), &utc(), &Week::default(), at(7));
        assert!(
            !p.events.iter().any(|e| e.key.starts_with("streak:")),
            "the Full Week achievement already says it"
        );
        assert!(p.events.iter().any(|e| e.key == "achievement:streak-7"));
    }

    #[test]
    fn a_day_that_counts_secures_the_streak_once_per_day() {
        let reviews: Vec<Review> = reviews_on(1, 5, 0);
        let p = compute("a", "a", &reviews, &utc(), &Week::default(), at(1));
        let secured = p.events.iter().find(|e| e.key == "secured:1").unwrap();
        assert_eq!(secured.title, "Streak secured!");
        assert_eq!(secured.body, "Today counts toward your 1-day streak.");
        let later = compute("a", "a", &reviews, &utc(), &Week::default(), at(2));
        assert!(!later.events.iter().any(|e| e.key.starts_with("secured:")));
        let goal = goal_event(1, 1, 12);
        assert_eq!(
            (goal.key.as_str(), goal.body.as_str()),
            ("goal:1", "1 card today in 12 minutes.")
        );
    }

    #[test]
    fn a_player_override_applies_from_its_time_and_can_be_handed_back() {
        let server = |at, minutes| StreakChange { at, minutes };
        let own = |at, minutes| PlayerStreakChange { at, minutes };
        let rule = StreakRule {
            changes: vec![server(100, 10), server(500, 15)],
            players: BTreeMap::from([(
                "a".to_string(),
                vec![own(200, Some(3)), own(400, Some(5)), own(600, None)],
            )]),
        };
        let a = rule.for_player("a");
        let minutes = |at| a.minutes_at(at);
        assert_eq!(
            [50, 100, 199, 200, 400, 500, 599, 600, 700].map(minutes),
            [0, 10, 10, 3, 5, 5, 5, 15, 15]
        );
        // Another player keeps the server's rule.
        assert_eq!(rule.for_player("b").changes, rule.changes);
        // An override before any server change still counts.
        let early = StreakRule {
            changes: vec![],
            players: BTreeMap::from([("a".to_string(), vec![own(10, Some(3))])]),
        };
        assert_eq!(early.for_player("a").minutes_at(9), 0);
        assert_eq!(early.for_player("a").minutes_at(10), 3);
    }

    #[test]
    fn a_zero_minimum_keeps_the_any_review_rule() {
        let reviews = study(&[(9, 1), (10, 12), (12, 1), (13, 3)]);
        for now in [at(12), at(13), at(14)] {
            let old = protected(&reviews, now, &[]);
            for rule in [
                vec![],
                vec![(0, 0)],
                vec![(0, 10), (utc().day_start_ms(5), 0)],
            ] {
                let new = ruled(&reviews, now, &rule);
                assert_eq!(
                    serde_json::to_value(&new).unwrap(),
                    serde_json::to_value(&old).unwrap(),
                    "{rule:?}"
                );
                assert_eq!(new.today.streak_minutes, 0);
            }
        }
        assert_eq!(StreakRule::default().minutes_at(i64::MAX), 0);
    }

    #[test]
    fn a_day_under_the_minimum_does_not_count() {
        let rule = [(0, 10)];
        let reviews = study(&[(10, 12), (11, 3), (12, 10)]);
        let p = ruled(&reviews, at(12), &rule);
        assert_eq!(p.streak, 1);
        assert_eq!(p.lifetime.best_streak, 1);
        assert_eq!(p.lifetime.days_active, 3, "short days are still study days");

        let reviews = study(&[(10, 12), (11, 12), (12, 6)]);
        let p = ruled(&reviews, at(12), &rule);
        assert_eq!(p.streak, 2, "today is open, not broken");
        assert!(p.at_risk);
        assert!(!p.today.counts());
        assert_eq!(serde_json::to_value(&p).unwrap()["streak_state"], "pending");
        assert_eq!((p.today.minutes, p.today.streak_minutes), (6, 10));
        assert_eq!(
            p.streak_warning.unwrap().title,
            "🔥 Your 2 day streak ends in 15h · 6 of 10 minutes today"
        );

        let done = ruled(&study(&[(10, 12), (11, 12), (12, 10)]), at(12), &rule);
        assert_eq!(done.streak, 3);
        assert!(!done.at_risk);
        assert!(done.today.counts());
        assert_eq!(
            serde_json::to_value(&done).unwrap()["streak_state"],
            "studied"
        );
    }

    #[test]
    fn days_before_the_minimum_keep_the_old_rule() {
        let start = utc().day_start_ms(11);
        let rule = [(start, 10)];
        let kept = ruled(
            &study(&[(9, 1), (10, 1), (11, 10), (12, 10)]),
            at(12),
            &rule,
        );
        assert_eq!(kept.streak, 4);
        let short = ruled(&study(&[(9, 1), (10, 1), (11, 1), (12, 10)]), at(12), &rule);
        assert_eq!(short.streak, 1);
        assert_eq!(short.lifetime.best_streak, 2);
        let midday = ruled(
            &study(&[(9, 1), (10, 1), (11, 1), (12, 10)]),
            at(12),
            &[(start + 1, 10)],
        );
        assert_eq!(
            midday.streak, 4,
            "a change during a day applies from the next one"
        );
    }

    #[test]
    fn a_later_change_only_affects_later_days() {
        let reviews = study(&[(9, 12), (10, 3), (11, 12), (12, 3), (13, 3)]);
        let ten = ruled(&reviews, at(13), &[(0, 10)]);
        assert_eq!(ten.streak, 0);
        let lowered = ruled(&reviews, at(13), &[(0, 10), (utc().day_start_ms(12), 2)]);
        assert_eq!(
            lowered.streak, 3,
            "day 10 stays missed, days 12 and 13 count"
        );
        assert_eq!(lowered.lifetime.best_streak, 3);
        assert_eq!(
            lowered
                .history
                .iter()
                .map(|day| day.streak)
                .collect::<Vec<_>>(),
            [1, 0, 1, 2, 3]
        );
        assert_eq!(lowered.today.streak_minutes, 2);
    }

    #[test]
    fn a_freeze_covers_a_day_under_the_minimum() {
        let mut reviews = perfect_history(&[1]);
        assert!(
            reviews.len() >= 3,
            "a perfect day takes at least three minutes"
        );
        reviews.extend(reviews_on(2, 1, 99_000));
        reviews.extend(minutes_on(3, 3));
        let p = ruled(&reviews, at(3), &[(0, 2)]);
        assert_eq!(p.streak, 2);
        assert!(p.history.iter().any(|day| day.frozen && day.reviews == 1));
        let cell = |day: i64| {
            let cell = p
                .heatmap
                .iter()
                .find(|c| c.date == date_string(day))
                .unwrap();
            serde_json::to_value(cell).unwrap()
        };
        let frozen = cell(2);
        assert_eq!(
            (
                &frozen["state"],
                &frozen["reviews"],
                &frozen["streak_minutes"]
            ),
            (
                &serde_json::json!("frozen"),
                &serde_json::json!(1),
                &serde_json::json!(2)
            ),
            "a frozen day reads as frozen although it had a review"
        );
        assert_eq!(cell(1)["state"], "met");
        assert_eq!(cell(3)["state"], "met");
        assert_eq!(cell(0)["state"], "none");
    }

    #[test]
    fn heatmap_days_carry_the_servers_verdict() {
        let rule = [(0, 10)];
        // A short day without a freeze breaks the streak and stays short.
        let p = ruled(&study(&[(10, 12), (11, 3), (12, 4)]), at(12), &rule);
        let state = |day: i64| {
            serde_json::to_value(
                p.heatmap
                    .iter()
                    .find(|c| c.date == date_string(day))
                    .unwrap(),
            )
            .unwrap()
        };
        assert_eq!(state(10)["state"], "met");
        assert_eq!(state(11)["state"], "short");
        assert_eq!(
            (&state(11)["minutes"], &state(11)["streak_minutes"]),
            (&serde_json::json!(3), &serde_json::json!(10))
        );
        assert_eq!(
            state(12)["state"],
            "short",
            "today is short until it is met"
        );
        assert_eq!(state(9)["state"], "none");
        let day = p
            .history
            .iter()
            .find(|d| d.date == date_string(11))
            .unwrap();
        assert_eq!(day.state, DayState::Short);
        assert_eq!(p.today.time_ms, 4 * 60_000);
        assert_eq!(p.today.starts_at, 12 * DAY_MS + 4 * 3_600_000);
        assert_eq!(
            DayState::of(2, 0, 0, true, true),
            DayState::Met,
            "a day that counted is met even when the clock skipped over its number"
        );
        assert_eq!(DayState::of(0, 0, 0, true, true), DayState::Skipped);
    }

    #[test]
    fn streak_achievements_wait_for_the_minimum() {
        let days: Vec<(i64, i64)> = (1..=7).map(|day| (day, 10)).collect();
        let mut reviews = study(&days[..6]);
        reviews.extend(minutes_on(7, 4));
        let week = |p: &Profile| {
            p.achievements
                .iter()
                .find(|a| a.title == "Full Week")
                .unwrap()
                .unlocked
                .is_some()
        };
        assert!(!week(&ruled(&reviews, at(7), &[(0, 10)])));
        assert!(week(&ruled(&study(&days), at(7), &[(0, 10)])));
    }
}
