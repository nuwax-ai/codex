//! Nuwax mark geometry for the welcome morph: the gradient pebble and the
//! four-pointed spark it blooms into. Both share one bounds box so the spark
//! reads at the icon's inset scale instead of filling the frame.

/// The pebble: an organic, slightly asymmetric rounded mark — broad dome on
/// top, full lower-left, tapered right, matching the app icon's silhouette.
pub(super) const PEBBLE: (&str, [f64; 4]) = (
    "M10 1.9C13.6 1.9 16.7 4.0 18.0 7.1C19.3 10.2 19.1 14.0 17.2 16.8C15.3 19.6 11.9 20.6 9.0 19.6C6.1 18.6 3.6 16.4 2.4 13.4C1.2 10.4 1.6 6.8 3.6 4.4C5.3 2.5 7.5 1.9 10 1.9Z",
    [0.7, 1.9, 19.6, 19.9],
);

/// The spark: a four-pointed star with concave sides, the icon's white core.
pub(super) const SPARK: (&str, [f64; 4]) = (
    "M10 3.4C10 7.03 12.79 10 16.2 10C12.79 10 10 13.63 10 16.6C10 13.63 7.21 10 3.8 10C7.21 10 10 7.03 10 3.4Z",
    [0.7, 1.9, 19.6, 19.9],
);
