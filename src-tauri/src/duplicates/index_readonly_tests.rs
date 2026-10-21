//! Explicit read-only numeric acceptance against existing fingerprints. No image IO,
//! cache writes, task execution or deletion plans are performed.
use super::*;
use rusqlite::{Connection, OpenFlags};
#[test]
#[ignore = "requires explicit ILLUTAG_READONLY_DATA_ROOT; fingerprint numbers only, no image IO or persistent writes"]
fn existing_numeric_candidates_complete_without_false_density() {
    let root = std::path::PathBuf::from(
        std::env::var_os("ILLUTAG_READONLY_DATA_ROOT").expect("explicit root required"),
    );
    assert!(root.is_absolute());
    let c = Connection::open_with_flags(
        root.join("cache/duplicates.sqlite"),
        OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    c.execute_batch("PRAGMA query_only=ON; PRAGMA cache_size=-8192; PRAGMA mmap_size=0; PRAGMA temp_store=MEMORY;").unwrap();
    let uri = super::super::workflow::readonly_uri(&root.join("illutag.sqlite")).unwrap();
    c.execute("ATTACH DATABASE ?1 AS library", [uri]).unwrap();
    let records = {
        let mut s=c.prepare("SELECT f.image_pk,f.image_order,f.size,f.width,f.height,f.sha,f.phash,f.dhash,f.color,f.low_info FROM fingerprints f CROSS JOIN library.images i ON i.image_pk=f.image_pk WHERE i.trashed=0 AND i.missing=0 AND COALESCE(i.source,'')<>'reference' AND f.sha IS NOT NULL ORDER BY f.image_pk").unwrap();
        let records = s
            .query_map([], |r| {
                let p = r.get::<_, Option<Vec<u8>>>(6)?;
                let d = r.get::<_, Option<Vec<u8>>>(7)?;
                let color = r.get::<_, Option<Vec<u8>>>(8)?;
                let width = r.get(3)?;
                let height = r.get(4)?;
                Ok(IndexRecord {
                    image_pk: r.get(0)?,
                    image_order: r.get(1)?,
                    file_size: r.get(2)?,
                    width,
                    height,
                    sha256: r.get::<_, Vec<u8>>(5)?.try_into().unwrap(),
                    near: match (p, d, color) {
                        (Some(p), Some(d), Some(color)) => Some(NearFingerprint {
                            phash: u64::from_le_bytes(p.try_into().unwrap()),
                            dhash: u64::from_le_bytes(d.try_into().unwrap()),
                            color: color.try_into().unwrap(),
                            width,
                            height,
                            low_information: r.get::<_, bool>(9)?,
                        }),
                        _ => None,
                    },
                })
            })
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        records
    };
    drop(c); // Do not retain a production reader while traversing numeric indexes.
    let start = std::time::Instant::now();
    let mut compared = 0_u64;
    let result = build_evidence_resumable(
        &records,
        Threshold::Standard,
        &EvidenceResume::default(),
        &mut || {
            if start.elapsed().as_secs() > 240 {
                Err("numeric acceptance deadline".into())
            } else {
                Ok(())
            }
        },
        &mut |_, _, kind| {
            // Collapsing equal size/hash units here is numeric simulation only; it
            // supplies no replacement proof and never creates a production plan.
            if kind == DuplicateKind::Exact {
                Ok(true)
            } else {
                compared += 1;
                Ok(false)
            }
        },
        &mut |_| Ok(()),
        &mut |_| Ok(()),
    )
    .unwrap();
    eprintln!("readonly numeric candidates: records={} comparisons={} dense={} elapsed_ms={} original_images_read=0 persistent_writes=0",records.len(),compared,result.too_dense_references.len(),start.elapsed().as_millis());
    assert!(
        result.too_dense_references.is_empty(),
        "genuine candidate density still needs investigation: {}",
        result.too_dense_references.len()
    );
}
