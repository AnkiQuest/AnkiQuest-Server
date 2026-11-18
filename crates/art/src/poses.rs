//! The pose list from `tests/fixtures/companion_poses.json`: the core poses
//! every live companion needs, and each extension's fallback.

use std::collections::BTreeMap;

/// Poses whose masters were named before the fixture: pose, then the
/// master's file name without `.png`. Wins store these file names (the
/// `art` field of the wins API), so they stay.
const LEGACY: [(&str, &str); 2] = [("happy", "celebrate"), ("asleep", "sleeping")];

/// The core poses and the extensions with their fallbacks.
pub struct Poses {
    core: Vec<String>,
    extensions: BTreeMap<String, String>,
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
        let extensions = value["extensions"]
            .as_object()
            .ok_or("no extensions")?
            .iter()
            .map(|(pose, fallback)| {
                let fallback = fallback.as_str().ok_or("a fallback is not text")?;
                Ok((pose.clone(), fallback.to_string()))
            })
            .collect::<Result<_, String>>()?;
        Ok(Poses { core, extensions })
    }

    /// Whether `pose` is a core pose.
    pub fn is_core(&self, pose: &str) -> bool {
        self.core.iter().any(|core| core == pose)
    }

    /// Every pose the fixture names, core first.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        let core = self.core.iter().map(String::as_str);
        core.chain(self.extensions.keys().map(String::as_str))
    }

    /// `pose`, then each fallback in turn, ending at a core pose. An unknown
    /// pose is its own chain.
    pub fn chain<'a>(&'a self, pose: &'a str) -> Vec<&'a str> {
        let mut chain = vec![pose];
        while let Some(next) = self.extensions.get(*chain.last().unwrap_or(&pose)) {
            // A loop in the fixture would never reach a core pose; stop at it.
            if chain.contains(&next.as_str()) {
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

/// The master's file name, without `.png`, for `pose`.
pub fn stem(pose: &str) -> &str {
    LEGACY
        .iter()
        .find(|(known, _)| *known == pose)
        .map_or(pose, |(_, stem)| stem)
}

/// The pose a master's file name, without `.png`, shows.
pub fn pose_of(stem: &str) -> &str {
    LEGACY
        .iter()
        .find(|(_, known)| *known == stem)
        .map_or(stem, |(pose, _)| pose)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> Poses {
        Poses::parse(include_str!("../../../tests/fixtures/companion_poses.json")).unwrap()
    }

    #[test]
    fn every_extension_reaches_a_core_pose() {
        let poses = fixture();
        for pose in poses.names() {
            let chain = poses.chain(pose);
            let last = chain.last().unwrap();
            assert!(poses.is_core(last), "{pose} ends at {last}: {chain:?}");
        }
    }

    #[test]
    fn extensions_fall_back_as_the_fixture_says() {
        let poses = fixture();
        assert_eq!(
            poses.chain("streak_legend"),
            ["streak_legend", "streak_blazing", "streak_hot", "streak"]
        );
        assert_eq!(poses.chain("frozen_3"), ["frozen_3", "frozen_2", "frozen"]);
        assert_eq!(poses.chain("winner"), ["winner", "happy"]);
        assert_eq!(poses.chain("unknown"), ["unknown"]);
        let has = |pose: &str| ["happy", "streak"].contains(&pose);
        assert_eq!(poses.resolve("streak_hot", has), Some("streak"));
        assert_eq!(poses.resolve("frozen", has), None);
    }

    #[test]
    fn legacy_masters_keep_their_file_names() {
        assert_eq!(stem("happy"), "celebrate");
        assert_eq!(stem("frozen"), "frozen");
        assert_eq!(stem("streak"), "streak");
        assert_eq!(pose_of("celebrate"), "happy");
        assert_eq!(pose_of("freeze"), "freeze");
        assert_eq!(pose_of("winner"), "winner");
    }
}
