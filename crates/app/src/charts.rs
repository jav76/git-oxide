use git_oxide_engine::models::ChurnTimeBucket;
use plotters::prelude::*;
use slint::{Image, Rgb8Pixel, SharedPixelBuffer};

/// Default width in pixels of the rasterized code churn timeline buffer.
pub const DEFAULT_CHART_WIDTH: u32 = 1040;

/// Default height in pixels of the rasterized code churn timeline buffer.
pub const DEFAULT_CHART_HEIGHT: u32 = 220;

const RGB_CHANNELS: usize = 3;
const GRID_HORIZONTAL_DIVISIONS: u32 = 4;
const BAR_PADDING_PX: i32 = 2;

const BG_COLOR: RGBColor = RGBColor(13, 17, 23);
const GRID_COLOR: RGBColor = RGBColor(38, 44, 54);
const BASELINE_COLOR: RGBColor = RGBColor(88, 96, 105);
const INSERTIONS_FILL: RGBColor = RGBColor(46, 160, 67);
const INSERTIONS_STROKE: RGBColor = RGBColor(63, 185, 80);
const DELETIONS_FILL: RGBColor = RGBColor(218, 54, 51);
const DELETIONS_STROKE: RGBColor = RGBColor(248, 81, 73);
const VELOCITY_LINE: RGBColor = RGBColor(88, 166, 255);

/// Rasterizes a time-bucketed churn and commit velocity chart into a Slint `Image`.
///
/// Must be called on the Slint UI thread because `slint::Image` is `!Send`.
pub fn render_churn_chart_image(buckets: &[ChurnTimeBucket], width: u32, height: u32) -> Image {
    let safe_width: u32 = width.max(64);
    let safe_height: u32 = height.max(48);
    let buffer: Vec<u8> = render_churn_chart_rgb_buffer(buckets, safe_width, safe_height);
    let pixel_buffer: SharedPixelBuffer<Rgb8Pixel> =
        SharedPixelBuffer::<Rgb8Pixel>::clone_from_slice(&buffer, safe_width, safe_height);
    Image::from_rgb8(pixel_buffer)
}

/// Rasterizes the dual-axis code churn (+insertions / -deletions) and commit velocity plot into an RGB8 buffer.
pub fn render_churn_chart_rgb_buffer(buckets: &[ChurnTimeBucket], width: u32, height: u32) -> Vec<u8> {
    let safe_width: u32 = width.max(64);
    let safe_height: u32 = height.max(48);
    let total_bytes: usize = (safe_width as usize) * (safe_height as usize) * RGB_CHANNELS;
    let mut buffer: Vec<u8> = vec![0u8; total_bytes];

    {
        let root = BitMapBackend::with_buffer(&mut buffer, (safe_width, safe_height)).into_drawing_area();
        let _ = root.fill(&BG_COLOR);

        let w_i32: i32 = safe_width as i32;
        let h_i32: i32 = safe_height as i32;
        let mid_y: i32 = h_i32 / 2;

        // Draw subtle horizontal grid lines
        for step in 1..GRID_HORIZONTAL_DIVISIONS {
            let y: i32 = (h_i32 * (step as i32)) / (GRID_HORIZONTAL_DIVISIONS as i32);
            let _ = root.draw(&PathElement::new(vec![(0, y), (w_i32, y)], GRID_COLOR.stroke_width(1)));
        }

        // Draw center zero-churn baseline
        let _ = root.draw(&PathElement::new(vec![(0, mid_y), (w_i32, mid_y)], BASELINE_COLOR.stroke_width(1)));

        if !buckets.is_empty() {
            let max_churn: u64 = buckets.iter().map(|b| b.insertions.max(b.deletions)).max().unwrap_or(1).max(1);
            let max_commits: u32 = buckets.iter().map(|b| b.commits).max().unwrap_or(1).max(1);

            let count: usize = buckets.len();
            let usable_half_h: f64 = f64::from((mid_y - 10).max(8));
            let slot_width: f64 = f64::from(w_i32 - 16) / (count as f64);
            let bar_width: i32 = ((slot_width as i32) - BAR_PADDING_PX).clamp(2, 28);

            let mut velocity_points: Vec<(i32, i32)> = Vec::with_capacity(count);

            for (idx, bucket) in buckets.iter().enumerate() {
                let center_x: i32 = 8 + ((idx as f64 + 0.5) * slot_width) as i32;
                let x0: i32 = center_x - (bar_width / 2);
                let x1: i32 = x0 + bar_width;

                // Above baseline: +insertions in emerald
                if bucket.insertions > 0 {
                    let ins_ratio: f64 = (bucket.insertions as f64 / max_churn as f64).sqrt();
                    let bar_h: i32 = ((ins_ratio * usable_half_h) as i32).max(2);
                    let y_top: i32 = (mid_y - bar_h).max(4);
                    let _ = root.draw(&Rectangle::new([(x0, y_top), (x1, mid_y)], INSERTIONS_FILL.filled()));
                    let _ =
                        root.draw(&PathElement::new(vec![(x0, y_top), (x1, y_top)], INSERTIONS_STROKE.stroke_width(1)));
                }

                // Below baseline: -deletions in coral
                if bucket.deletions > 0 {
                    let del_ratio: f64 = (bucket.deletions as f64 / max_churn as f64).sqrt();
                    let bar_h: i32 = ((del_ratio * usable_half_h) as i32).max(2);
                    let y_bottom: i32 = (mid_y + bar_h).min(h_i32 - 4);
                    let _ = root.draw(&Rectangle::new([(x0, mid_y + 1), (x1, y_bottom)], DELETIONS_FILL.filled()));
                    let _ = root.draw(&PathElement::new(
                        vec![(x0, y_bottom), (x1, y_bottom)],
                        DELETIONS_STROKE.stroke_width(1),
                    ));
                }

                let commit_ratio: f64 = f64::from(bucket.commits) / f64::from(max_commits);
                let vel_y: i32 = h_i32 - 8 - ((commit_ratio * f64::from((h_i32 - 20).max(10))) as i32);
                velocity_points.push((center_x, vel_y));
            }

            if velocity_points.len() >= 2 {
                let _ = root.draw(&PathElement::new(velocity_points.clone(), VELOCITY_LINE.stroke_width(2)));
            }
            for point in velocity_points {
                let _ = root.draw(&Circle::new(point, 2, VELOCITY_LINE.filled()));
            }
        }

        let _ = root.present();
    }

    buffer
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chart_rasterizer_handles_empty_and_populated_series_without_panicking() {
        let empty_buf: Vec<u8> = render_churn_chart_rgb_buffer(&[], 320, 120);
        assert_eq!(empty_buf.len(), 320 * 120 * 3);

        let sample_buckets: Vec<ChurnTimeBucket> = vec![
            ChurnTimeBucket {
                period_label: "2026-10".to_owned(),
                start_timestamp: 1_760_000_000,
                commits: 12,
                insertions: 450,
                deletions: 120,
                net_lines: 330,
            },
            ChurnTimeBucket {
                period_label: "2026-11".to_owned(),
                start_timestamp: 1_762_600_000,
                commits: 24,
                insertions: 980,
                deletions: 310,
                net_lines: 670,
            },
        ];
        let populated_buf: Vec<u8> = render_churn_chart_rgb_buffer(&sample_buckets, 320, 120);
        assert_eq!(populated_buf.len(), 320 * 120 * 3);
        assert_ne!(empty_buf, populated_buf);
        let (pixels, _): (&[[u8; 3]], &[u8]) = populated_buf.as_chunks::<3>();
        assert!(
            pixels.contains(&[INSERTIONS_FILL.0, INSERTIONS_FILL.1, INSERTIONS_FILL.2]),
            "expected emerald insertion bar pixels in rendered buffer"
        );
        assert!(
            pixels.contains(&[DELETIONS_FILL.0, DELETIONS_FILL.1, DELETIONS_FILL.2]),
            "expected coral deletion bar pixels in rendered buffer"
        );
    }
}
