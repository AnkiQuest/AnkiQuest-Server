//! The `intensity_cases` in `tests/fixtures/review_heatmap.json`, through [`heatmap_intensity`].

use ankiquest_rules::heatmap_intensity;
use serde_json::Value;
use std::path::PathBuf;

fn fixture() -> Value {
    let path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/review_heatmap.json");
    let text = std::fs::read_to_string(&path).unwrap();
    serde_json::from_str(&text).unwrap()
}

#[test]
fn every_intensity_case_matches() {
    let fixture = fixture();
    let cases = fixture["intensity_cases"].as_array().unwrap();
    assert!(!cases.is_empty());
    for case in cases {
        let average = case["daily_average"].as_f64().unwrap();
        let levels: Vec<u64> = case["counts"]
            .as_array()
            .unwrap()
            .iter()
            .map(|count| {
                let count = u32::try_from(count.as_u64().unwrap()).unwrap();
                u64::from(heatmap_intensity(count, average))
            })
            .collect();
        let expected: Vec<u64> = case["levels"]
            .as_array()
            .unwrap()
            .iter()
            .map(|level| level.as_u64().unwrap())
            .collect();
        assert_eq!(levels, expected, "daily_average {average}");
    }
}
