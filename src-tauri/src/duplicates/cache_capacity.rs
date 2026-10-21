//! Disk allowance covers fingerprints, the published snapshot, a draft, and WAL.
//! It is independent of the working-memory limit and does not preallocate files.
use rusqlite::Connection;

pub(super) const BASE_BYTES: u64 = 512 * 1024 * 1024;
pub(super) const MAX_BYTES: u64 = 8 * 1024 * 1024 * 1024;
pub(super) const MIN_FREE_BYTES: u64 = 2 * 1024 * 1024 * 1024;
const BYTES_PER_IMAGE: u64 = 2048;
const STEP: u64 = 64 * 1024 * 1024;
const WAL_RESERVE: u64 = 32 * 1024 * 1024;
const WRITE_RESERVE: u64 = 32 * 1024 * 1024;

pub(super) fn allowance(images: u64, allocated: u64, free: u64) -> u64 {
    let desired = BASE_BYTES
        .saturating_add(images.saturating_mul(BYTES_PER_IMAGE))
        .div_ceil(STEP)
        .saturating_mul(STEP)
        .min(MAX_BYTES);
    // Existing bytes have already been paid for. Only growth consumes free space.
    desired.min(allocated.saturating_add(free.saturating_sub(MIN_FREE_BYTES)))
}

pub(super) fn soft_limit(allowance: u64) -> u64 {
    allowance.saturating_sub(WAL_RESERVE + WRITE_RESERVE)
}
pub(super) fn max_pages(allowance: u64, page_size: u64, current: u64) -> u64 {
    allowance
        .saturating_sub(WAL_RESERVE)
        .checked_div(page_size)
        .unwrap_or(0)
        .max(current)
        .max(1)
}

pub(super) fn configure(conn: &Connection, images: u64, free: u64) -> Result<u64, String> {
    let page_size: u64 = conn
        .query_row("PRAGMA page_size", [], |r| r.get(0))
        .map_err(|e| e.to_string())?;
    let pages: u64 = conn
        .query_row("PRAGMA page_count", [], |r| r.get(0))
        .map_err(|e| e.to_string())?;
    let total = allowance(images, pages.saturating_mul(page_size), free);
    let max_pages = max_pages(total, page_size, pages);
    let actual: u64 = conn
        .query_row(&format!("PRAGMA max_page_count={max_pages}"), [], |r| {
            r.get(0)
        })
        .map_err(|e| format!("无法设置查重缓存容量：{e}"))?;
    if actual != max_pages {
        return Err("查重缓存容量设置未生效，保留已有结果".into());
    }
    Ok(total)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn storage_allowance_scales_with_library_and_accounts_for_existing_bytes() {
        let gib = 1024 * 1024 * 1024;
        assert_eq!(allowance(0, 0, 100 * gib), BASE_BYTES);
        let large = allowance(839_334, 503_308_288, 232 * gib);
        assert!(large >= 2 * gib && large <= 3 * gib);
        assert_eq!(allowance(u64::MAX, 0, u64::MAX), MAX_BYTES);
        assert_eq!(
            allowance(
                839_334,
                480 * 1024 * 1024,
                MIN_FREE_BYTES + 128 * 1024 * 1024
            ),
            608 * 1024 * 1024
        );
        assert_eq!(
            allowance(839_334, 480 * 1024 * 1024, MIN_FREE_BYTES - 1),
            480 * 1024 * 1024
        );
        assert!(soft_limit(large) < large - WAL_RESERVE);
    }
    #[test]
    fn every_connection_replaces_the_old_page_ceiling_and_respects_real_page_size() {
        for page_size in [4096, 16384] {
            let f = crate::duplicates::tests::Fixture::new(0);
            let c = Connection::open(f.root.join("capacity.sqlite")).unwrap();
            c.execute_batch(&format!("PRAGMA page_size={page_size}; CREATE TABLE sample(x BLOB); PRAGMA max_page_count=16;")).unwrap();
            let failure = c
                .execute("INSERT INTO sample VALUES(zeroblob(524288))", [])
                .unwrap_err();
            assert_eq!(
                failure.sqlite_error_code(),
                Some(rusqlite::ErrorCode::DiskFull)
            );
            let total = configure(&c, 839_334, 100 * 1024 * 1024 * 1024).unwrap();
            let real_size: u64 = c.query_row("PRAGMA page_size", [], |r| r.get(0)).unwrap();
            assert_eq!(real_size, page_size);
            let max: u64 = c
                .query_row("PRAGMA max_page_count", [], |r| r.get(0))
                .unwrap();
            assert_eq!(max, (total - WAL_RESERVE) / real_size);
            c.execute("INSERT INTO sample VALUES(zeroblob(524288))", [])
                .unwrap();
            assert_eq!(
                c.query_row("SELECT length(x) FROM sample", [], |r| r.get::<_, i64>(0))
                    .unwrap(),
                524288
            );
        }
    }
}
