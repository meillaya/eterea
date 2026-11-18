//! Graph and calendar data building for the service layer.
//!
//! Pure aggregation and layout; no UI or shell dependency. The data layer
//! returns deterministic results so the same archive always renders the same
//! graph positions and calendar grid.

use super::AppServices;
use crate::types::{
    CalendarData, CalendarDay, CalendarDayCount, CalendarRequest, CalendarYearData,
    CalendarYearRequest, GraphData, GraphEdge, GraphNode, GraphRequest,
};
use anyhow::{bail, Context, Result};
use chrono::{Datelike, Duration as ChronoDuration, NaiveDate, TimeZone, Utc};
use std::collections::{BTreeMap, HashMap};

impl AppServices {
    /// Deterministic tag graph for the graph screen.
    ///
    /// Nodes are the top tags ordered by bookmark count then name, laid out on
    /// the unit circle starting at the top and moving clockwise. Edges are tag
    /// pairs sharing a bookmark, strongest first, capped by the request.
    pub fn graph_data(&self, request: &GraphRequest) -> Result<GraphData> {
        let request = request.bounded();
        let tags = self
            .db
            .get_top_tags(request.max_nodes)
            .context("failed to load graph tags")?;
        let index: HashMap<&str, usize> = tags
            .iter()
            .enumerate()
            .map(|(node_index, (tag, _))| (tag.as_str(), node_index))
            .collect();

        let mut pairs = Vec::new();
        if request.max_edges > 0 && !tags.is_empty() {
            // Restrict the pair scan to the selected node set: with a thousand
            // tags the archive-wide pair table is much larger than the graph.
            let node_tags: Vec<String> = tags.iter().map(|(tag, _)| tag.clone()).collect();
            for pair in self
                .db
                .get_tag_cooccurrences_for_tags(&node_tags, request.max_edges)
                .context("failed to load graph edges")?
            {
                let (Some(&source), Some(&target)) = (
                    index.get(pair.first_tag.as_str()),
                    index.get(pair.second_tag.as_str()),
                ) else {
                    continue;
                };
                pairs.push((source, target, pair.bookmark_count));
            }
        }

        let nodes = layout_nodes(&tags, &pairs);
        let edges = pairs
            .into_iter()
            .map(|(source, target, weight)| GraphEdge {
                source,
                target,
                weight,
            })
            .collect();
        Ok(GraphData { nodes, edges })
    }

    /// One year of per-day saved-post counts for the year overview.
    pub fn calendar_year_data(&self, request: &CalendarYearRequest) -> Result<CalendarYearData> {
        if !(1970..=2100).contains(&request.year) {
            anyhow::bail!("calendar year {} is out of range", request.year);
        }
        let start = Utc
            .with_ymd_and_hms(request.year, 1, 1, 0, 0, 0)
            .single()
            .ok_or_else(|| anyhow::anyhow!("invalid calendar year {}", request.year))?;
        let end = Utc
            .with_ymd_and_hms(request.year + 1, 1, 1, 0, 0, 0)
            .single()
            .ok_or_else(|| anyhow::anyhow!("invalid calendar year {}", request.year))?;
        let days = self
            .db
            .get_calendar_year_counts(start.timestamp(), end.timestamp())
            .context("failed to load calendar year counts")?
            .into_iter()
            .map(|(date, bookmark_count)| CalendarDayCount {
                date,
                bookmark_count,
            })
            .collect();
        Ok(CalendarYearData {
            year: request.year,
            days,
        })
    }

    /// Monday-first calendar grid for one UTC month.
    ///
    /// The grid covers whole weeks and includes days from adjacent months, so
    /// every saved post in the visible cells keeps its mark. A month that needs
    /// six Monday-first rows gets six; every other month gets the five-row grid
    /// from the design.
    pub fn calendar_data(&self, request: &CalendarRequest) -> Result<CalendarData> {
        let (year, month) = parse_month(&request.month)?;
        let first = NaiveDate::from_ymd_opt(year, month, 1)
            .ok_or_else(|| anyhow::anyhow!("invalid calendar month {}-{:02}", year, month))?;
        let next_month_first = if month == 12 {
            NaiveDate::from_ymd_opt(year + 1, 1, 1)
        } else {
            NaiveDate::from_ymd_opt(year, month + 1, 1)
        }
        .ok_or_else(|| anyhow::anyhow!("invalid calendar month {}-{:02}", year, month))?;

        let days_in_month = next_month_first
            .signed_duration_since(first)
            .num_days()
            .max(0);
        let monday_offset = i64::from(first.weekday().num_days_from_monday());
        let needed_weeks = ((monday_offset + days_in_month + 6) / 7) as usize;
        let weeks = needed_weeks.max(5);
        let grid_start = first - ChronoDuration::days(monday_offset);
        let grid_end = grid_start + ChronoDuration::days((weeks * 7 - 1) as i64);

        let from_timestamp = Utc
            .from_utc_datetime(&grid_start.and_hms_opt(0, 0, 0).unwrap_or_default())
            .timestamp();
        let to_timestamp = Utc
            .from_utc_datetime(&grid_end.and_hms_opt(23, 59, 59).unwrap_or_default())
            .timestamp();

        let entries = self
            .db
            .get_bookmark_day_entries(from_timestamp, to_timestamp)
            .context("failed to load calendar day entries")?;
        let mut ids_by_day = BTreeMap::<String, Vec<String>>::new();
        for (day, bookmark_id) in entries {
            ids_by_day.entry(day).or_default().push(bookmark_id);
        }

        let mut days = Vec::with_capacity(weeks * 7);
        let mut cursor = grid_start;
        for _ in 0..weeks * 7 {
            let date = cursor.format("%Y-%m-%d").to_string();
            let bookmark_ids = ids_by_day.remove(&date).unwrap_or_default();
            days.push(CalendarDay {
                date,
                bookmark_count: bookmark_ids.len() as i64,
                bookmark_ids,
            });
            cursor += ChronoDuration::days(1);
        }

        Ok(CalendarData {
            month: format!("{year:04}-{month:02}"),
            grid_start: grid_start.format("%Y-%m-%d").to_string(),
            grid_end: grid_end.format("%Y-%m-%d").to_string(),
            weeks,
            days,
        })
    }
}

/// Deterministic force-directed layout on the unit disc.
///
/// Co-occurring tags attract, every node repels its neighbours through a grid
/// (so the pass stays near-linear for a thousand nodes), popular tags get a
/// gentle pull to the middle, and the final cloud is normalised to a unit
/// disc. The seed is the tag name, not its rank, so the same archive always
/// produces the same constellation.
fn layout_nodes(tags: &[(String, i64)], edges: &[(usize, usize, i64)]) -> Vec<GraphNode> {
    let count = tags.len();
    if count == 0 {
        return Vec::new();
    }
    let mut positions: Vec<(f32, f32)> = tags
        .iter()
        .map(|(tag, _)| {
            let hash = tag.bytes().fold(0xcbf29ce484222325u64, |hash, byte| {
                hash.wrapping_mul(1099511628211).wrapping_add(byte as u64)
            });
            let angle = (hash % 10_000) as f32 / 10_000.0 * std::f32::consts::TAU;
            let radius = 0.18 + 0.7 * ((hash >> 20) % 1_000) as f32 / 1_000.0;
            (angle.cos() * radius, angle.sin() * radius)
        })
        .collect();
    if count == 1 {
        return vec![GraphNode {
            tag: tags[0].0.clone(),
            bookmark_count: tags[0].1,
            x: 0.0,
            y: 0.0,
        }];
    }

    let iterations = if count > 600 {
        80
    } else if count > 250 {
        130
    } else {
        200
    };
    let ideal = (2.0 / (count as f32).sqrt()).max(0.02);
    let mut force = vec![(0.0f32, 0.0f32); count];

    for step in 0..iterations {
        let temperature = 0.1 * (1.0 - step as f32 / iterations as f32) + 0.004;
        let cell = (ideal * 3.0).max(0.04);
        let mut buckets: HashMap<(i32, i32), Vec<usize>> = HashMap::with_capacity(count);
        for (index, (x, y)) in positions.iter().enumerate() {
            buckets
                .entry(((x / cell).floor() as i32, (y / cell).floor() as i32))
                .or_default()
                .push(index);
        }
        for index in 0..count {
            let (x, y) = positions[index];
            let mut sum = (0.0, 0.0);
            let key = ((x / cell).floor() as i32, (y / cell).floor() as i32);
            for dx in -1..=1 {
                for dy in -1..=1 {
                    let Some(neighbours) = buckets.get(&(key.0 + dx, key.1 + dy)) else {
                        continue;
                    };
                    for &other in neighbours {
                        if other == index {
                            continue;
                        }
                        let (ox, oy) = positions[other];
                        let (rx, ry) = (x - ox, y - oy);
                        let distance = (rx * rx + ry * ry).sqrt().max(0.001);
                        let repulsion = ideal * ideal / distance;
                        sum.0 += rx / distance * repulsion;
                        sum.1 += ry / distance * repulsion;
                    }
                }
            }
            force[index] = sum;
        }
        for &(source, target, weight) in edges {
            if source >= count || target >= count {
                continue;
            }
            let (sx, sy) = positions[source];
            let (tx, ty) = positions[target];
            let (dx, dy) = (tx - sx, ty - sy);
            let distance = (dx * dx + dy * dy).sqrt().max(0.001);
            let strength = (weight.max(1) as f32).ln().max(0.4) * 0.25;
            let attraction = distance * distance / ideal * strength;
            let (ux, uy) = (dx / distance, dy / distance);
            force[source].0 += ux * attraction;
            force[source].1 += uy * attraction;
            force[target].0 -= ux * attraction;
            force[target].1 -= uy * attraction;
        }
        for index in 0..count {
            let (x, y) = positions[index];
            let weight = (tags[index].1.max(1) as f32).ln().max(0.3);
            let gravity = 0.05 * weight;
            let (fx, fy) = force[index];
            let (fx, fy) = (fx - x * gravity, fy - y * gravity);
            let length = (fx * fx + fy * fy).sqrt().max(0.0001);
            let step = length.min(temperature);
            positions[index] = (x + fx / length * step, y + fy / length * step);
        }
    }

    let extent = positions
        .iter()
        .map(|(x, y)| (x * x + y * y).sqrt())
        .fold(0.0f32, f32::max)
        .max(0.001);
    let scale = 0.92 / extent;
    tags.iter()
        .zip(positions)
        .map(|((tag, bookmark_count), (x, y))| GraphNode {
            tag: tag.clone(),
            bookmark_count: *bookmark_count,
            x: x * scale,
            y: y * scale,
        })
        .collect()
}

fn parse_month(month: &str) -> Result<(i32, u32)> {
    let trimmed = month.trim();
    let (year, month) = trimmed
        .split_once('-')
        .ok_or_else(|| anyhow::anyhow!("invalid calendar month {trimmed}: expected YYYY-MM"))?;
    if year.len() != 4 || month.len() != 2 {
        bail!("invalid calendar month {trimmed}: expected YYYY-MM");
    }
    let year: i32 = year
        .parse()
        .with_context(|| format!("invalid calendar year in {trimmed}"))?;
    let month: u32 = month
        .parse()
        .with_context(|| format!("invalid calendar month in {trimmed}"))?;
    if !(1..=12).contains(&month) {
        bail!("invalid calendar month in {trimmed}: month must be 1-12");
    }
    Ok((year, month))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn imported_services() -> AppServices {
        let services = AppServices::open_memory().expect("in-memory services should open");
        services
            .import_content(
                "bookmarks.json",
                include_str!("../../../../../fixtures/import/bookmarks.json"),
            )
            .expect("fixture import should succeed");
        services
    }

    #[test]
    fn graph_handles_twelve_hundred_tags_with_cooccurrence_edges() {
        // Synthetic archive at the data-layer cap: 1,200 tags, each post
        // carrying two tags from a rolling window so the restricted pair query
        // finds a dense edge set.
        let services = AppServices::open_memory().expect("in-memory services should open");
        let mut json = String::from("[");
        for index in 0..1_200usize {
            if index > 0 {
                json.push(',');
            }
            let first = format!("tag{index:04}");
            let second = format!("tag{:04}", (index + 7) % 1_200);
            json.push_str(&format!(
                r#"{{"tweet_url":"https://x.com/scale/status/{index}","full_text":"scale fixture {index} #{first} #{second}","tweeted_at":"2024-05-01T12:00:00Z","screen_name":"scale","name":"Scale"}}"#
            ));
        }
        json.push(']');
        services
            .import_content("scale.json", &json)
            .expect("synthetic import should succeed");

        let started = std::time::Instant::now();
        let graph = services
            .graph_data(&GraphRequest::new(1_200, 6_000))
            .expect("large graph should build");
        let elapsed = started.elapsed();
        assert_eq!(graph.nodes.len(), 1_200);
        assert!(!graph.edges.is_empty(), "expected co-occurrence edges");
        assert!(graph.edges.len() <= 6_000);
        assert!(
            graph
                .nodes
                .iter()
                .all(|node| node.x.abs() <= 1.0 && node.y.abs() <= 1.0),
            "laid-out nodes must stay in the unit disc"
        );
        eprintln!("1200-tag graph built in {elapsed:?} (debug)");
    }

    #[test]
    fn graph_layout_is_deterministic_and_bounded() {
        let services = imported_services();
        let first = services.graph_data(&GraphRequest::default()).unwrap();
        let second = services.graph_data(&GraphRequest::default()).unwrap();
        assert_eq!(first, second, "graph layout must be deterministic");

        assert!(!first.nodes.is_empty());
        assert_eq!(first.nodes[0].tag, "architecture");
        let mut extent: f32 = 0.0;
        for node in &first.nodes {
            let radius = (node.x * node.x + node.y * node.y).sqrt();
            extent = extent.max(radius);
            assert!(radius <= 0.921, "node sits outside the unit disc: {radius}");
        }
        assert!(extent > 0.7, "layout should use the disc: {extent}");
        // Fixture bookmark 1 carries rust + architecture, so that pair exists.
        let pair = first
            .edges
            .iter()
            .find(|edge| {
                let tags = (&first.nodes[edge.source].tag, &first.nodes[edge.target].tag);
                tags == (&"architecture".to_string(), &"rust".to_string())
                    || tags == (&"rust".to_string(), &"architecture".to_string())
            })
            .expect("rust/architecture edge should exist");
        assert_eq!(pair.weight, 1);
    }

    #[test]
    fn graph_respects_requested_caps() {
        let services = imported_services();
        let graph = services.graph_data(&GraphRequest::new(1, 0)).unwrap();
        assert_eq!(graph.nodes.len(), 1);
        assert!(graph.edges.is_empty());

        let empty = services.graph_data(&GraphRequest::new(0, 0)).unwrap();
        assert!(empty.nodes.is_empty());
        assert!(empty.edges.is_empty());
    }

    #[test]
    fn calendar_grid_is_monday_first_five_weeks_and_carries_marks() {
        let services = imported_services();
        let calendar = services
            .calendar_data(&CalendarRequest::new("2026-05"))
            .unwrap();

        assert_eq!(calendar.month, "2026-05");
        assert_eq!(calendar.grid_start, "2026-04-27");
        assert_eq!(calendar.grid_end, "2026-05-31");
        assert_eq!(calendar.weeks, 5);
        assert_eq!(calendar.days.len(), 35);
        assert_eq!(calendar.days[0].date, "2026-04-27");

        let marked: Vec<&CalendarDay> = calendar
            .days
            .iter()
            .filter(|day| day.bookmark_count > 0)
            .collect();
        assert_eq!(marked.len(), 3);
        assert_eq!(marked[0].date, "2026-05-01");
        assert_eq!(marked[0].bookmark_count, 1);
        assert!(!marked[0].bookmark_ids.is_empty());
        assert_eq!(marked[1].date, "2026-05-02");
        assert_eq!(marked[2].date, "2026-05-03");
    }

    #[test]
    fn calendar_rejects_malformed_months() {
        let services = imported_services();
        for month in ["", "2026", "2026-5", "2026-13", "abcd-ef"] {
            assert!(
                services
                    .calendar_data(&CalendarRequest::new(month))
                    .is_err(),
                "{month} should be rejected"
            );
        }
    }
}
