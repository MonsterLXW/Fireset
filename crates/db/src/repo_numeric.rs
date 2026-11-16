//! Filling the numeric condition columns (migration 0005) for rows that already exist.
//!
//! New rows get their numbers when they are inserted (repo_records.rs). This is for the rows
//! extracted before the columns existed. It reads the three text columns with `units.rs` and
//! fills only columns that are still NULL, so it can be run again at any time and never
//! overwrites a value.

use crate::{units, Db};
use serde::Serialize;
use shared::Result;
use sqlx::FromRow;
use uuid::Uuid;

/// How many examples of unreadable text a report carries.
const EXAMPLES: usize = 20;

#[derive(Debug, FromRow)]
struct TextRow {
    id: Uuid,
    oxygen_concentration: Option<String>,
    pressure: Option<String>,
    airflow: Option<String>,
    has_oxygen: bool,
    has_pressure: bool,
    has_airflow: bool,
}

/// One of the three quantities across all rows.
#[derive(Debug, Default, Clone, Serialize)]
pub struct QuantityCount {
    /// Rows where the paper's text column is not empty.
    pub with_text: usize,
    /// Of those, rows the parser reads as one value or range.
    pub readable: usize,
    /// Rows whose numeric columns are already filled.
    pub already_filled: usize,
    /// Rows this run fills (or, in a dry run, would fill).
    pub to_fill: usize,
}

/// A piece of text the parser left alone, and how many rows have exactly this text.
#[derive(Debug, Clone, Serialize)]
pub struct Unreadable {
    pub column: &'static str,
    pub text: String,
    pub rows: usize,
}

#[derive(Debug, Default, Clone, Serialize)]
pub struct BackfillReport {
    pub dry_run: bool,
    pub rows: usize,
    /// Rows with at least one numeric column to fill.
    pub rows_to_fill: usize,
    /// Rows actually updated. Always 0 in a dry run.
    pub rows_updated: usize,
    pub oxygen: QuantityCount,
    pub pressure: QuantityCount,
    pub airflow: QuantityCount,
    /// Distinct texts that could not be read, most common first, capped at 20.
    pub unreadable: Vec<Unreadable>,
    /// How many distinct unreadable texts there are in all.
    pub unreadable_distinct: usize,
}

impl Db {
    /// Reads every condition's text and fills the numeric columns that are still NULL.
    /// With `dry_run` nothing is written; the report says what would be.
    pub async fn backfill_numeric_conditions(&self, dry_run: bool) -> Result<BackfillReport> {
        let rows: Vec<TextRow> = sqlx::query_as(
            "SELECT id, oxygen_concentration, pressure, airflow,
                    (oxygen_pct_min IS NOT NULL OR oxygen_pct_max IS NOT NULL) AS has_oxygen,
                    (pressure_kpa_min IS NOT NULL OR pressure_kpa_max IS NOT NULL) AS has_pressure,
                    (airflow_cm_s_min IS NOT NULL OR airflow_cm_s_max IS NOT NULL) AS has_airflow
             FROM conditions
             ORDER BY created_at ASC, id ASC",
        )
        .fetch_all(self.pool())
        .await?;

        let mut report = BackfillReport { dry_run, rows: rows.len(), ..Default::default() };
        let mut unreadable: Vec<Unreadable> = vec![];
        let mut note = |column: &'static str, text: &str| match unreadable.iter_mut().find(|u| u.column == column && u.text == text) {
            Some(u) => u.rows += 1,
            None => unreadable.push(Unreadable { column, text: text.to_string(), rows: 1 }),
        };
        let stated = |text: &Option<String>| text.as_deref().map(str::trim).filter(|t| !t.is_empty()).map(str::to_string);

        for row in &rows {
            let (oxygen, pressure, airflow) = (stated(&row.oxygen_concentration), stated(&row.pressure), stated(&row.airflow));
            let n = units::parse_condition(oxygen.as_deref(), pressure.as_deref(), airflow.as_deref());
            let mut fill = false;
            for (count, text, read, has, column) in [
                (&mut report.oxygen, &oxygen, n.oxygen_pct, row.has_oxygen, "oxygen_concentration"),
                (&mut report.pressure, &pressure, n.pressure_kpa, row.has_pressure, "pressure"),
                (&mut report.airflow, &airflow, n.airflow_cm_s, row.has_airflow, "airflow"),
            ] {
                if let Some(text) = text {
                    count.with_text += 1;
                    match read {
                        Some(_) => count.readable += 1,
                        None => note(column, text),
                    }
                }
                if has {
                    count.already_filled += 1;
                } else if read.is_some() {
                    count.to_fill += 1;
                    fill = true;
                }
            }
            if !fill {
                continue;
            }
            report.rows_to_fill += 1;
            if dry_run {
                continue;
            }
            // COALESCE keeps anything already there: only NULL columns are filled.
            let done = sqlx::query(
                "UPDATE conditions SET
                    oxygen_pct_min = COALESCE(oxygen_pct_min, ($2::float8)::numeric),
                    oxygen_pct_max = COALESCE(oxygen_pct_max, ($3::float8)::numeric),
                    pressure_kpa_min = COALESCE(pressure_kpa_min, ($4::float8)::numeric),
                    pressure_kpa_max = COALESCE(pressure_kpa_max, ($5::float8)::numeric),
                    airflow_cm_s_min = COALESCE(airflow_cm_s_min, ($6::float8)::numeric),
                    airflow_cm_s_max = COALESCE(airflow_cm_s_max, ($7::float8)::numeric)
                 WHERE id = $1",
            )
            .bind(row.id)
            .bind(n.oxygen_pct.filter(|_| !row.has_oxygen).map(|r| r.min))
            .bind(n.oxygen_pct.filter(|_| !row.has_oxygen).map(|r| r.max))
            .bind(n.pressure_kpa.filter(|_| !row.has_pressure).map(|r| r.min))
            .bind(n.pressure_kpa.filter(|_| !row.has_pressure).map(|r| r.max))
            .bind(n.airflow_cm_s.filter(|_| !row.has_airflow).map(|r| r.min))
            .bind(n.airflow_cm_s.filter(|_| !row.has_airflow).map(|r| r.max))
            .execute(self.pool())
            .await?;
            report.rows_updated += done.rows_affected() as usize;
        }

        unreadable.sort_by(|a, b| b.rows.cmp(&a.rows).then(a.column.cmp(b.column)).then(a.text.cmp(&b.text)));
        report.unreadable_distinct = unreadable.len();
        unreadable.truncate(EXAMPLES);
        report.unreadable = unreadable;
        Ok(report)
    }
}
