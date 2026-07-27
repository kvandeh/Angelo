use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use tokio::runtime::Runtime;
use turso::{Builder, Connection};

use crate::mutate::Mutant;
use crate::runner::BatchOutcome;

const SCHEMA: &str = include_str!("db/schema.sql");

/// The one place async lives: turso's API is async-only, so this struct owns a
/// tokio runtime and `block_on`s every call.
pub struct Database {
    runtime: Runtime,
    connection: Connection,
}

impl Database {
    pub fn open(directory: &Path) -> Result<Self> {
        fs::create_dir_all(directory)
            .with_context(|| format!("creating {}", directory.display()))?;
        let path = directory.join("angelo.db");
        let path = path.to_str().context("database path is not valid UTF-8")?;

        let runtime = new_runtime()?;
        let database = runtime
            .block_on(Builder::new_local(path).build())
            .with_context(|| format!("opening {path}"))?;
        let connection = database.connect().context("connecting to the database")?;
        runtime
            .block_on(connection.execute_batch(SCHEMA))
            .context("creating the schema")?;

        Ok(Database {
            runtime,
            connection,
        })
    }

    pub fn mutant_count(&self) -> Result<i64> {
        let mut rows = self
            .runtime
            .block_on(self.connection.query("SELECT COUNT(*) FROM mutant", ()))?;
        let row = self
            .runtime
            .block_on(rows.next())?
            .context("count query returned no row")?;
        Ok(row.get(0)?)
    }

    pub fn insert_mutants(&self, mutants: &[Mutant]) -> Result<()> {
        for mutant in mutants {
            let file = mutant
                .file
                .to_str()
                .context("file path is not valid UTF-8")?;
            self.runtime.block_on(self.connection.execute(
                "INSERT INTO mutant (file, line, byte_start, byte_end, original, replacement)
                 VALUES (?, ?, ?, ?, ?, ?)",
                (
                    file,
                    mutant.line as i64,
                    mutant.byte_start as i64,
                    mutant.byte_end as i64,
                    mutant.original.as_str(),
                    mutant.replacement.as_str(),
                ),
            ))?;
        }
        Ok(())
    }

    /// Keep a random `keep` of the enumerated mutants and delete the rest.
    ///
    /// This drops mutants from the pool, it does not merely stop running them:
    /// the surviving rows ARE the study, so the score is an estimate over a
    /// random sample of the whole codebase rather than a complete census of
    /// one arbitrary corner of it. Returns how many were dropped.
    ///
    /// The shuffle is seeded from the current time, so re-running the same
    /// project picks a different sample each time.
    pub fn sample_down_to(&self, keep: usize) -> Result<usize> {
        let mut ids = self.all_ids()?;
        if ids.len() <= keep {
            return Ok(0);
        }
        let mut random = get_random();
        // Fisher-Yates, then everything past `keep` goes.
        for index in (1..ids.len()).rev() {
            ids.swap(index, (random.next() % (index as u64 + 1)) as usize);
        }
        let dropped = &ids[keep..];
        for id in dropped {
            self.runtime.block_on(
                self.connection
                    .execute("DELETE FROM mutant WHERE id = ?", (*id,)),
            )?;
        }
        Ok(dropped.len())
    }

    fn all_ids(&self) -> Result<Vec<i64>> {
        let mut rows = self.runtime.block_on(
            self.connection
                .query("SELECT id FROM mutant ORDER BY id", ()),
        )?;
        let mut ids = Vec::new();
        while let Some(row) = self.runtime.block_on(rows.next())? {
            ids.push(row.get(0)?);
        }
        Ok(ids)
    }

    pub fn pending_mutants(&self) -> Result<Vec<Mutant>> {
        self.mutants_where("status = 'pending'")
    }

    pub fn survivors(&self) -> Result<Vec<Mutant>> {
        self.mutants_where("status = 'survived'")
    }

    pub fn record_batch(&self, outcome: &BatchOutcome) -> Result<()> {
        self.runtime.block_on(self.connection.execute(
            "INSERT INTO execution (duration_ms, exit_code, failed_tests) VALUES (?, ?, ?)",
            (
                outcome.duration_ms as i64,
                outcome.exit_code,
                outcome.failed_tests.join("\n"),
            ),
        ))?;
        let execution_id = self.connection.last_insert_rowid();
        for (mutant_id, status) in &outcome.verdicts {
            self.runtime.block_on(self.connection.execute(
                "UPDATE mutant SET status = ?, execution_id = ? WHERE id = ?",
                (status.as_str(), execution_id, *mutant_id),
            ))?;
        }
        Ok(())
    }

    pub fn record_survived_unrun(&self, mutants: &[Mutant]) -> Result<()> {
        for mutant in mutants {
            self.runtime.block_on(self.connection.execute(
                "UPDATE mutant SET status = 'survived' WHERE id = ?",
                (mutant.id,),
            ))?;
        }
        Ok(())
    }

    pub fn status_counts(&self) -> Result<Vec<(String, i64)>> {
        let mut rows = self.runtime.block_on(self.connection.query(
            "SELECT status, COUNT(*) FROM mutant GROUP BY status ORDER BY status",
            (),
        ))?;
        let mut counts = Vec::new();
        while let Some(row) = self.runtime.block_on(rows.next())? {
            counts.push((row.get(0)?, row.get(1)?));
        }
        Ok(counts)
    }

    fn mutants_where(&self, condition: &str) -> Result<Vec<Mutant>> {
        let sql = format!(
            "SELECT id, file, line, byte_start, byte_end, original, replacement
             FROM mutant WHERE {condition} ORDER BY id"
        );
        let mut rows = self.runtime.block_on(self.connection.query(&sql, ()))?;
        let mut mutants = Vec::new();
        while let Some(row) = self.runtime.block_on(rows.next())? {
            mutants.push(Mutant {
                id: row.get(0)?,
                file: PathBuf::from(row.get::<String>(1)?),
                line: row.get::<i64>(2)? as u32,
                byte_start: row.get::<i64>(3)? as usize,
                byte_end: row.get::<i64>(4)? as usize,
                original: row.get(5)?,
                replacement: row.get(6)?,
            });
        }
        Ok(mutants)
    }
}

pub struct CoverageRow {
    pub file: String,
    pub context_id: i64,
    pub context: String,
    pub numbits: Vec<u8>,
}

/// Read coverage.py's data file, it is SQLite, so turso opens it directly.
pub fn read_coverage_rows(path: &Path) -> Result<Vec<CoverageRow>> {
    let path = path.to_str().context("coverage path is not valid UTF-8")?;
    let runtime = new_runtime()?;
    let database = runtime
        .block_on(Builder::new_local(path).build())
        .with_context(|| format!("opening {path}"))?;
    let connection = database
        .connect()
        .context("connecting to the coverage data")?;

    let mut rows = runtime.block_on(connection.query(
        "SELECT file.path, context.id, context.context, line_bits.numbits
         FROM line_bits
         JOIN file ON file.id = line_bits.file_id
         JOIN context ON context.id = line_bits.context_id",
        (),
    ))?;
    let mut result = Vec::new();
    while let Some(row) = runtime.block_on(rows.next())? {
        result.push(CoverageRow {
            file: row.get(0)?,
            context_id: row.get(1)?,
            context: row.get(2)?,
            numbits: row.get(3)?,
        });
    }
    Ok(result)
}

/// Small deterministic PRNG. A dependency would buy nothing here: sampling
/// needs spread, not cryptographic quality.
#[derive(Debug, Clone, Copy)]
pub struct Xorshift(u64);

impl Xorshift {
    pub fn seeded(seed: u64) -> Xorshift {
        // 0 is a fixed point of xorshift, so never start there.
        Xorshift(seed.wrapping_mul(6364136223846793005).wrapping_add(1))
    }

    pub fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
}

pub fn get_random() -> Xorshift {
    let seed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(1000);
    Xorshift::seeded(seed)
}

fn new_runtime() -> Result<Runtime> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("starting tokio runtime")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn the_shuffle_spreads_across_the_whole_pool() {
        let mut random = get_random();

        let mut ids: Vec<usize> = (0..1000).collect();
        for index in (1..ids.len()).rev() {
            ids.swap(index, (random.next() % (index as u64 + 1)) as usize);
        }
        let sample: HashSet<usize> = ids[..100].iter().copied().collect();
        assert_eq!(sample.len(), 100, "the sample must not repeat ids");

        let from_last_half = sample.iter().filter(|id| **id >= 500).count();
        assert!(
            (25..=75).contains(&from_last_half),
            "expected a roughly even split, got {from_last_half}/100 from the second half"
        );
    }

    #[test]
    fn the_same_seed_samples_the_same_way() {
        let first: Vec<u64> = (0..5).map(|_| Xorshift::seeded(42).next()).collect();
        assert!(first.iter().all(|value| *value == first[0]));
        assert_ne!(Xorshift::seeded(42).next(), Xorshift::seeded(43).next());
    }
}
