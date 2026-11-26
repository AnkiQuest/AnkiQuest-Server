//! The review heatmap's colour levels. The cases are `intensity_cases` in
//! `tests/fixtures/review_heatmap.json`.

/// The colour level of a day with `count` reviews, or review cards due, against the collection's
/// reviews per study day: 0 for none, then up to half the average, up to the average, up to
/// twice it, and more (4). An average under one counts as one, so a new collection does not
/// colour every day at the top level.
#[cfg_attr(feature = "ffi", uniffi::export)]
pub fn heatmap_intensity(count: u32, daily_average: f64) -> u8 {
    let average = daily_average.max(1.0);
    let count = f64::from(count);
    if count <= 0.0 {
        return 0;
    }
    if count <= average / 2.0 {
        return 1;
    }
    if count <= average {
        return 2;
    }
    if count <= average * 2.0 {
        return 3;
    }
    4
}
