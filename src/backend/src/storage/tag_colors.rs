//! Reader-chosen tag colors, validated as `#rrggbb`.

use super::database::Database;
use crate::models::TagColor;
use crate::{Error, Result};
use eterea_logic::is_valid_hex_color;
use rusqlite::params;

impl Database {
    /// Validate and store a tag color. Returns the stored row.
    pub fn set_tag_color(&self, tag: &str, color: &str) -> Result<TagColor> {
        let tag = tag.trim().trim_start_matches('#');
        if tag.is_empty() {
            return Err(Error::Other("tag color needs a tag name".to_string()));
        }
        let color = color.trim();
        if !is_valid_hex_color(color) {
            return Err(Error::Other(format!(
                "invalid tag color '{color}': expected #rrggbb"
            )));
        }

        self.conn.execute(
            r#"INSERT INTO tag_colors (tag, color) VALUES (?1, ?2)
               ON CONFLICT(tag) DO UPDATE SET color = excluded.color"#,
            params![tag, color],
        )?;
        Ok(TagColor {
            tag: tag.to_string(),
            color: color.to_string(),
        })
    }

    /// One tag's color, case-insensitively resolved.
    pub fn get_tag_color(&self, tag: &str) -> Result<Option<TagColor>> {
        let tag = tag.trim().trim_start_matches('#');
        let result = self.conn.query_row(
            "SELECT tag, color FROM tag_colors WHERE tag = ?1",
            params![tag],
            |row| {
                Ok(TagColor {
                    tag: row.get(0)?,
                    color: row.get(1)?,
                })
            },
        );
        match result {
            Ok(color) => Ok(Some(color)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(error) => Err(error.into()),
        }
    }

    /// Every stored tag color, ordered by tag name for deterministic rendering.
    pub fn tag_colors(&self) -> Result<Vec<TagColor>> {
        let mut stmt = self
            .conn
            .prepare("SELECT tag, color FROM tag_colors ORDER BY tag COLLATE NOCASE")?;
        let colors = stmt
            .query_map([], |row| {
                Ok(TagColor {
                    tag: row.get(0)?,
                    color: row.get(1)?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(colors)
    }

    /// Remove a tag color. Returns whether a row was removed.
    pub fn clear_tag_color(&self, tag: &str) -> Result<bool> {
        let tag = tag.trim().trim_start_matches('#');
        let removed = self
            .conn
            .execute("DELETE FROM tag_colors WHERE tag = ?1", params![tag])?;
        Ok(removed > 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tag_colors_are_validated_and_case_insensitive() {
        let db = Database::open_memory().unwrap();

        let stored = db.set_tag_color("Rust", "#2447B8").unwrap();
        assert_eq!(stored.tag, "Rust");
        assert_eq!(db.get_tag_color("rust").unwrap().unwrap().color, "#2447B8");
        assert_eq!(db.tag_colors().unwrap().len(), 1);

        db.set_tag_color("rust", "#b4452f").unwrap();
        assert_eq!(db.tag_colors().unwrap().len(), 1);
        assert_eq!(db.get_tag_color("RUST").unwrap().unwrap().color, "#b4452f");

        assert!(db.set_tag_color("rust", "b4452f").is_err());
        assert!(db.set_tag_color("rust", "#fff").is_err());
        assert!(db.set_tag_color("", "#b4452f").is_err());

        assert!(db.clear_tag_color("rust").unwrap());
        assert!(!db.clear_tag_color("rust").unwrap());
    }
}
