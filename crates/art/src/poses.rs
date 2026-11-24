//! The pose list from `tests/fixtures/companion_poses.json`: the core moods,
//! the moment and share-card pictures and the launcher icon moods every live
//! companion needs, and the extensions. Every item but a core mood names the
//! item a companion without its master shows instead.

use std::collections::BTreeMap;

/// What an item of the list is. The first three are the core set a
/// companion needs to go live.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// One of the fixture's `core` poses: a streak mood's picture.
    CoreMood,
    /// A moment or share-card picture (`pictures`).
    Picture,
    /// A launcher icon mood (`icons`).
    Icon,
    /// A tier or variant of a core item (`extensions`).
    Extension,
}

impl Kind {
    /// Every kind, in the order reports list them.
    pub const ALL: [Kind; 4] = [Kind::CoreMood, Kind::Picture, Kind::Icon, Kind::Extension];

    /// The kind's name in reports and JSON.
    pub fn name(self) -> &'static str {
        match self {
            Kind::CoreMood => "core_mood",
            Kind::Picture => "picture",
            Kind::Icon => "icon",
            Kind::Extension => "extension",
        }
    }

    /// Whether a companion needs this kind to go live.
    pub fn is_core(self) -> bool {
        self != Kind::Extension
    }
}

/// Core moods that show another core mood until their master is drawn:
/// mood, then the mood shown instead. AnkiQuest-Art's pose list gives these
/// as `core_fallbacks`; the fixture does not have them yet.
const CORE_FALLBACKS: [(&str, &str); 2] = [("happy", "hopeful"), ("streak", "hopeful")];

/// The whole list: each item's kind, and each fallback.
pub struct Poses {
    core: Vec<String>,
    pictures: BTreeMap<String, String>,
    icons: BTreeMap<String, String>,
    extensions: BTreeMap<String, String>,
}

/// The fallbacks under `key`, empty when the fixture has no such key:
/// `pictures` and `icons` came after `core` and `extensions`.
fn fallbacks(value: &serde_json::Value, key: &str) -> Result<BTreeMap<String, String>, String> {
    let Some(entries) = value.get(key) else {
        return Ok(BTreeMap::new());
    };
    let entries = entries
        .as_object()
        .ok_or(format!("{key} is not an object"))?;
    entries
        .iter()
        .map(|(item, fallback)| {
            let fallback = fallback
                .as_str()
                .ok_or(format!("{key}: the fallback of {item} is not text"))?;
            Ok((item.clone(), fallback.to_string()))
        })
        .collect()
}

impl Poses {
    /// Reads the fixture's JSON.
    pub fn parse(json: &str) -> Result<Poses, String> {
        let value: serde_json::Value = serde_json::from_str(json).map_err(|e| e.to_string())?;
        let core = value["core"]
            .as_array()
            .ok_or("no core poses")?
            .iter()
            .map(|pose| {
                pose.as_str()
                    .map(String::from)
                    .ok_or("a core pose is not text")
            })
            .collect::<Result<_, _>>()?;
        if value.get("extensions").is_none() {
            return Err("no extensions".into());
        }
        Ok(Poses {
            core,
            pictures: fallbacks(&value, "pictures")?,
            icons: fallbacks(&value, "icons")?,
            extensions: fallbacks(&value, "extensions")?,
        })
    }

    /// Whether `pose` is a core mood.
    pub fn is_core(&self, pose: &str) -> bool {
        self.core.iter().any(|core| core == pose)
    }

    /// Whether the fixture lists pictures and icons; a fixture from before
    /// the matrix lists only core moods and extensions.
    pub fn has_core_set(&self) -> bool {
        !self.pictures.is_empty() || !self.icons.is_empty()
    }

    /// The kind of `item`, or `None` when the fixture does not list it.
    pub fn kind(&self, item: &str) -> Option<Kind> {
        if self.is_core(item) {
            return Some(Kind::CoreMood);
        }
        [
            (Kind::Picture, &self.pictures),
            (Kind::Icon, &self.icons),
            (Kind::Extension, &self.extensions),
        ]
        .into_iter()
        .find(|(_, items)| items.contains_key(item))
        .map(|(kind, _)| kind)
    }

    /// The item a companion without `item` shows instead; `None` for an
    /// unknown item and a core mood without one ([`CORE_FALLBACKS`], when
    /// the list has the mood it names).
    pub fn fallback(&self, item: &str) -> Option<&str> {
        if self.is_core(item) {
            return CORE_FALLBACKS
                .iter()
                .find(|(mood, shown)| *mood == item && self.is_core(shown))
                .map(|(_, shown)| *shown);
        }
        [&self.pictures, &self.icons, &self.extensions]
            .into_iter()
            .find_map(|items| items.get(item))
            .map(String::as_str)
    }

    /// The items of one kind, in the fixture's order for core moods and by
    /// name for the others.
    pub fn of(&self, kind: Kind) -> Vec<&str> {
        fn keys(items: &BTreeMap<String, String>) -> Vec<&str> {
            items.keys().map(String::as_str).collect()
        }
        match kind {
            Kind::CoreMood => self.core.iter().map(String::as_str).collect(),
            Kind::Picture => keys(&self.pictures),
            Kind::Icon => keys(&self.icons),
            Kind::Extension => keys(&self.extensions),
        }
    }

    /// Every item, in [`Kind::ALL`] order.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        Kind::ALL.into_iter().flat_map(|kind| self.of(kind))
    }

    /// `pose`, then each fallback in turn, ending at a core mood without a
    /// fallback. An unknown pose is its own chain.
    pub fn chain<'a>(&'a self, pose: &'a str) -> Vec<&'a str> {
        let mut chain = vec![pose];
        while let Some(next) = self.fallback(chain.last().unwrap_or(&pose)) {
            // A loop in the fixture would never reach a core mood; stop at it.
            if chain.contains(&next) {
                break;
            }
            chain.push(next);
        }
        chain
    }

    /// The first pose in `pose`'s chain that `has` accepts.
    pub fn resolve<'a>(&'a self, pose: &'a str, has: impl Fn(&str) -> bool) -> Option<&'a str> {
        self.chain(pose).into_iter().find(|pose| has(pose))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> Poses {
        Poses::parse(include_str!("../../../tests/fixtures/companion_poses.json")).unwrap()
    }

    #[test]
    fn every_item_reaches_a_core_mood() {
        let poses = fixture();
        for pose in poses.names() {
            let chain = poses.chain(pose);
            let last = chain.last().unwrap();
            assert!(poses.is_core(last), "{pose} ends at {last}: {chain:?}");
        }
    }

    #[test]
    fn items_have_one_kind() {
        let poses = fixture();
        let mut names: Vec<_> = poses.names().collect();
        let count = names.len();
        names.sort_unstable();
        names.dedup();
        assert_eq!(count, names.len(), "an item is listed twice");
    }

    #[test]
    fn the_core_set_lists_pictures_and_icons() {
        // hill, 2026-11-21: the core set is the core moods, the moment and
        // share-card pictures and the launcher icon moods.
        let poses = fixture();
        assert!(poses.has_core_set());
        assert_eq!(poses.kind("face"), Some(Kind::CoreMood));
        assert_eq!(poses.kind("place_1"), Some(Kind::Picture));
        assert_eq!(poses.kind("freeze"), Some(Kind::Picture));
        assert_eq!(poses.kind("icon_happy"), Some(Kind::Icon));
        assert_eq!(poses.kind("frozen_2"), Some(Kind::Extension));
        assert_eq!(poses.kind("study"), None);
        assert_eq!(poses.chain("icon_happy"), ["icon_happy", "icon", "face"]);
        assert_eq!(poses.chain("freeze"), ["freeze", "frozen"]);
        assert!(Kind::ALL.iter().filter(|kind| kind.is_core()).count() == 3);
    }

    #[test]
    fn an_old_fixture_has_no_pictures_or_icons() {
        let old = r#"{"core": ["happy"], "extensions": {"winner": "happy"}}"#;
        let poses = Poses::parse(old).unwrap();
        assert!(!poses.has_core_set());
        assert_eq!(poses.names().collect::<Vec<_>>(), ["happy", "winner"]);
        assert!(Poses::parse(r#"{"core": []}"#).is_err());
    }

    #[test]
    fn extensions_fall_back_as_the_fixture_says() {
        let poses = fixture();
        assert_eq!(
            poses.chain("streak_legend"),
            [
                "streak_legend",
                "streak_blazing",
                "streak_hot",
                "streak",
                "hopeful"
            ]
        );
        assert_eq!(poses.chain("frozen_3"), ["frozen_3", "frozen_2", "frozen"]);
        assert_eq!(poses.chain("unknown"), ["unknown"]);
        let has = |pose: &str| ["happy", "streak"].contains(&pose);
        assert_eq!(poses.resolve("streak_hot", has), Some("streak"));
        assert_eq!(poses.resolve("frozen", has), None);
    }

    #[test]
    fn happy_and_streak_fall_back_to_hopeful() {
        let poses = fixture();
        assert_eq!(poses.chain("winner"), ["winner", "happy", "hopeful"]);
        assert_eq!(
            poses.chain("streak_hot"),
            ["streak_hot", "streak", "hopeful"]
        );
        assert_eq!(poses.chain("hopeful"), ["hopeful"]);
        assert_eq!(poses.kind("happy"), Some(Kind::CoreMood));
        let has = |pose: &str| pose == "hopeful";
        assert_eq!(poses.resolve("welcome", has), Some("hopeful"));
    }
}
