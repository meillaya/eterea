//! Graph and calendar data building for the service layer.
//!
//! Pure aggregation and layout; no UI or shell dependency. The data layer
//! returns deterministic results so the same archive always renders the same
//! graph positions and calendar grid.

use super::AppServices;
use crate::types::{
    CalendarData, CalendarDay, CalendarRequest, GraphData, GraphEdge, GraphNode, GraphRequest,
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
        let nodes = layout_nodes(&tags);

        let mut edges = Vec::new();
        if request.max_edges > 0 && !nodes.is_empty() {
            let index: HashMap<&str, usize> = nodes
                .iter()
                .enumerate()
                .map(|(node_index, node)| (node.tag.as_str(), node_index))
                .collect();
            // Ask for more pairs than needed because pairs touching tags that
            // did not make the node cut are filtered out below.
            let scan_limit = request.max_edges.saturating_mul(8);
            let pairs = self
                .db
                .get_tag_cooccurrences(scan_limit)
                .context("failed to load graph edges")?;
            for pair in pairs {
                if edges.len() >= request.max_edges {
                    break;
                }
                let (Some(&source), Some(&target)) = (
                    index.get(pair.first_tag.as_str()),
                    index.get(pair.second_tag.as_str()),
                ) else {
                    continue;
                };
                edges.push(GraphEdge {
                    source,
                    target,
                    weight: pair.bookmark_count,
                });
            }
        }

        Ok(GraphData { nodes, edges })
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

/// Place tags on the unit circle: first tag at the top, clockwise, equal
/// spacing. A single tag sits at the centre.
fn layout_nodes(tags: &[(String, i64)]) -> Vec<GraphNode> {
    let node_count = tags.len();
    tags.iter()
        .enumerate()
        .map(|(index, (tag, bookmark_count))| {
            let (x, y) = if node_count <= 1 {
                (0.0, 0.0)
            } else {
                let step = std::f32::consts::TAU / node_count as f32;
                let angle = -std::f32::consts::FRAC_PI_2 + step * index as f32;
                (angle.cos(), angle.sin())
            };
            GraphNode {
                tag: tag.clone(),
                bookmark_count: *bookmark_count,
                x,
                y,
            }
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
    fn graph_layout_is_deterministic_and_unit_circle() {
        let services = imported_services();
        let first = services.graph_data(&GraphRequest::default()).unwrap();
        let second = services.graph_data(&GraphRequest::default()).unwrap();
        assert_eq!(first, second, "graph layout must be deterministic");

        assert!(!first.nodes.is_empty());
        assert_eq!(first.nodes[0].tag, "architecture");
        for node in &first.nodes {
            assert!((node.x * node.x + node.y * node.y - 1.0).abs() < 0.001);
        }
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
