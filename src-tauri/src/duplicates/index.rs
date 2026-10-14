//! Exact content units and bounded multi-index pHash search. Groups are stars, not components.
use super::fingerprint::NearFingerprint;
use std::collections::{HashMap, HashSet};

pub const MEMORY_BUDGET_BYTES: u64 = 512 * 1024 * 1024;
pub const MAX_DISTINCT_CANDIDATES: usize = 50_000;

#[derive(Clone, Debug)]
pub struct IndexRecord {
    pub image_pk: i64,
    /// Rank of the public image ID in ascending order, supplied by the main repository.
    pub image_order: u32,
    pub file_size: u64,
    pub width: u32,
    pub height: u32,
    pub sha256: [u8; 32],
    pub near: Option<NearFingerprint>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Threshold { Strict, Standard, Loose }

impl Threshold {
    pub fn limits(self) -> (u32, u32, u32) {
        match self { Self::Strict => (2, 8, 16), Self::Standard => (4, 12, 24), Self::Loose => (6, 16, 32) }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DuplicateKind { Exact, Near }

#[derive(Clone, Debug, PartialEq)]
pub struct MatchMetrics {
    pub phash_distance: u32,
    pub dhash_distance: u32,
    /// Mean absolute RGB difference, normalized to 0..1, not a similarity probability.
    pub color_distance: f32,
    pub aspect_ratio_difference: f32,
}

#[derive(Clone, Debug)]
pub struct DuplicateMember {
    pub record_index: usize,
    /// The reference has no metrics; exact copies also need no approximate metrics.
    pub metrics: Option<MatchMetrics>,
}

#[derive(Clone, Debug)]
pub struct DuplicateGroup {
    pub kind: DuplicateKind,
    pub reference: usize,
    /// Includes the reference as the first member.
    pub members: Vec<DuplicateMember>,
}

#[derive(Clone, Debug, Default)]
pub struct GroupingSummary {
    pub group_count: u64,
    pub too_dense_references: Vec<usize>,
    pub low_information_skipped: u64,
    pub estimated_memory_bytes: u64,
}

#[derive(Clone, Debug, Default)]
pub struct GroupingResult {
    pub groups: Vec<DuplicateGroup>,
    pub too_dense_references: Vec<usize>,
    pub low_information_skipped: u64,
    pub estimated_memory_bytes: u64,
}

/// Includes input records, content/feature grouping maps at their construction peaks,
/// four postings, buckets, query stamps, quality order, and one worst-case output group.
/// Production uses the streaming API so all emitted groups are not retained here.
pub fn estimate_memory_bytes(record_count: usize) -> u64 {
    (record_count as u64).saturating_mul(480).saturating_add(16 * 1024 * 1024)
}

pub fn compare_near(a: &NearFingerprint, b: &NearFingerprint, threshold: Threshold) -> Option<MatchMetrics> {
    if a.low_information || b.low_information || a.width == 0 || a.height == 0 || b.width == 0 || b.height == 0 { return None; }
    let (phash_limit, dhash_limit, color_limit) = threshold.limits();
    let phash_distance = (a.phash ^ b.phash).count_ones();
    if phash_distance > phash_limit { return None; }
    let dhash_distance = (a.dhash ^ b.dhash).count_ones();
    if dhash_distance > dhash_limit { return None; }
    let left = u64::from(a.width) * u64::from(b.height);
    let right = u64::from(b.width) * u64::from(a.height);
    let aspect_ratio_difference = left.abs_diff(right) as f64 / left.max(right) as f64;
    if aspect_ratio_difference > 0.02 { return None; }
    let color_sum: u32 = a.color.iter().zip(b.color).map(|(a, b)| u32::from(a.abs_diff(b))).sum();
    if color_sum > color_limit * 48 { return None; }
    Some(MatchMetrics { phash_distance, dhash_distance, color_distance: color_sum as f32 / (48.0 * 255.0), aspect_ratio_difference: aspect_ratio_difference as f32 })
}

struct ContentUnit { representative: usize, members: Vec<usize> }
struct FeatureGroup { near: NearFingerprint, units: Vec<usize> }

fn quality_order(a: usize, b: usize, records: &[IndexRecord]) -> std::cmp::Ordering {
    let a = &records[a]; let b = &records[b];
    let area = |value: &IndexRecord| value.near.as_ref().map(|n| u64::from(n.width) * u64::from(n.height)).unwrap_or_else(|| u64::from(value.width) * u64::from(value.height));
    area(b).cmp(&area(a)).then_with(|| b.file_size.cmp(&a.file_size)).then_with(|| a.image_order.cmp(&b.image_order)).then_with(|| a.image_pk.cmp(&b.image_pk))
}

/// Four flattened 16-bit posting indexes. For radius <=6 at least one segment
/// differs by <=1 bit, so probing each segment plus its 16 one-bit neighbors is complete.
struct MihIndex { offsets: Vec<u32>, postings: Vec<u32>, stamps: Vec<u32>, epoch: u32 }

impl MihIndex {
    fn new(phashes: &[u64], checkpoint: &mut dyn FnMut() -> Result<(), String>) -> Result<Self, String> {
        if phashes.len() > u32::MAX as usize { return Err("index_too_large".into()); }
        let mut offsets = vec![0_u32; 4 * 65_537];
        for (index, &phash) in phashes.iter().enumerate() {
            if index % 8192 == 0 { checkpoint()?; }
            for segment in 0..4 {
                let value = ((phash >> (16 * segment)) & 0xffff) as usize;
                offsets[segment * 65_537 + value + 1] += 1;
            }
        }
        for segment in 0..4 {
            let base = segment * 65_537;
            for value in 1..65_537 { offsets[base + value] += offsets[base + value - 1]; }
        }
        let mut positions = offsets.clone();
        let mut postings = vec![0; 4 * phashes.len()];
        for (index, &phash) in phashes.iter().enumerate() {
            if index % 8192 == 0 { checkpoint()?; }
            for segment in 0..4 {
                let value = ((phash >> (16 * segment)) & 0xffff) as usize;
                let position = &mut positions[segment * 65_537 + value];
                postings[segment * phashes.len() + *position as usize] = index as u32;
                *position += 1;
            }
        }
        Ok(Self { offsets, postings, stamps: vec![0; phashes.len()], epoch: 0 })
    }

    /// None means the distinct-fingerprint budget was exceeded, not an empty query.
    fn candidates(&mut self, phash: u64, checkpoint: &mut dyn FnMut() -> Result<(), String>) -> Result<Option<Vec<u32>>, String> {
        self.epoch = self.epoch.wrapping_add(1);
        if self.epoch == 0 { self.stamps.fill(0); self.epoch = 1; }
        let mut result = Vec::new();
        let mut visited = 0;
        for segment in 0..4 {
            checkpoint()?;
            let value = ((phash >> (segment * 16)) & 0xffff) as usize;
            for variation in 0..17 {
                let bucket = if variation == 0 { value } else { value ^ (1 << (variation - 1)) };
                let base = segment * 65_537;
                let start = self.offsets[base + bucket] as usize;
                let end = self.offsets[base + bucket + 1] as usize;
                let posting_base = segment * self.stamps.len();
                for &candidate in &self.postings[posting_base + start..posting_base + end] {
                    visited += 1;
                    if visited % 8192 == 0 { checkpoint()?; }
                    let stamp = &mut self.stamps[candidate as usize];
                    if *stamp == self.epoch { continue; }
                    *stamp = self.epoch;
                    result.push(candidate);
                    if result.len() > MAX_DISTINCT_CANDIDATES { return Ok(None); }
                }
            }
        }
        Ok(Some(result))
    }
}

pub fn build_groups_each(
    records: &[IndexRecord], threshold: Threshold,
    checkpoint: &mut dyn FnMut() -> Result<(), String>,
    on_group: &mut dyn FnMut(DuplicateGroup) -> Result<(), String>,
) -> Result<GroupingSummary, String> {
    let estimated_memory_bytes = estimate_memory_bytes(records.len());
    if estimated_memory_bytes > MEMORY_BUDGET_BYTES { return Err("memory_budget".into()); }
    checkpoint()?;
    let mut units: Vec<ContentUnit> = Vec::with_capacity(records.len());
    let mut content_map = HashMap::with_capacity(records.len());
    for (index, record) in records.iter().enumerate() {
        if index % 8192 == 0 { checkpoint()?; }
        let unit_index = *content_map.entry((record.file_size, record.sha256)).or_insert_with(|| {
            units.push(ContentUnit { representative: index, members: Vec::with_capacity(1) }); units.len() - 1
        });
        let unit = &mut units[unit_index];
        unit.members.push(index);
        if quality_order(index, unit.representative, records).is_lt() { unit.representative = index; }
    }
    drop(content_map);
    let mut order: Vec<usize> = (0..units.len()).collect();
    order.sort_unstable_by(|&a, &b| quality_order(units[a].representative, units[b].representative, records));
    let mut summary = GroupingSummary { estimated_memory_bytes, low_information_skipped: records.iter().filter(|r| r.near.as_ref().is_some_and(|n| n.low_information)).count() as u64, ..Default::default() };
    for &unit_index in &order {
        let unit = &mut units[unit_index];
        if unit.members.len() < 2 { continue; }
        checkpoint()?;
        unit.members.sort_unstable_by(|&a, &b| quality_order(a, b, records));
        let members = unit.members.iter().map(|&record_index| DuplicateMember { record_index, metrics: None }).collect();
        on_group(DuplicateGroup { kind: DuplicateKind::Exact, reference: unit.representative, members })?;
        summary.group_count += 1;
    }

    let mut feature_groups: Vec<FeatureGroup> = Vec::with_capacity(units.len());
    // References compare/hash their values while borrowing stable input records.
    // Avoid copying an 80-byte fingerprint into every rounded HashMap bucket.
    let mut feature_map: HashMap<&NearFingerprint, usize> = HashMap::with_capacity(units.len());
    let mut unit_feature = vec![None; units.len()];
    for &unit_index in &order {
        if unit_index % 8192 == 0 { checkpoint()?; }
        let unit = &units[unit_index];
        let near = unit.members.iter().filter_map(|&i| records[i].near.as_ref()).find(|n| !n.low_information);
        let Some(near) = near else { continue; };
        let feature_index = *feature_map.entry(near).or_insert_with(|| {
            feature_groups.push(FeatureGroup { near: near.clone(), units: Vec::with_capacity(1) }); feature_groups.len() - 1
        });
        feature_groups[feature_index].units.push(unit_index);
        unit_feature[unit_index] = Some(feature_index);
    }
    drop(feature_map);
    let phashes: Vec<u64> = feature_groups.iter().map(|f| f.near.phash).collect();
    let mut index = MihIndex::new(&phashes, checkpoint)?;
    drop(phashes);
    let mut covered = vec![false; units.len()];
    let mut dense_hashes = HashSet::new();
    for &anchor in &order {
        if covered[anchor] { continue; }
        let Some(feature_index) = unit_feature[anchor] else { continue; };
        checkpoint()?;
        let reference = units[anchor].representative;
        let near = &feature_groups[feature_index].near;
        let candidates = if dense_hashes.contains(&near.phash) { None } else { index.candidates(near.phash, checkpoint)? };
        let Some(candidates) = candidates else {
            // Equal pHashes query identical buckets: memoize density to avoid repeated huge scans.
            dense_hashes.insert(near.phash);
            summary.too_dense_references.push(reference);
            continue;
        };
        let mut unseen: Vec<(usize, MatchMetrics)> = Vec::new();
        let mut bridge: Option<(usize, MatchMetrics)> = None;
        let mut inspected_units = 0_usize;
        for candidate_feature in candidates {
            let feature = &feature_groups[candidate_feature as usize];
            let Some(metrics) = compare_near(near, &feature.near, threshold) else { continue; };
            for &candidate in &feature.units {
                inspected_units += 1;
                if inspected_units % 4096 == 0 { checkpoint()?; }
                if candidate == anchor { continue; }
                if !covered[candidate] { unseen.push((candidate, metrics.clone())); }
                else {
                    let prefer = match &bridge {
                        None => true,
                        Some((old, old_metrics)) => metrics.phash_distance.cmp(&old_metrics.phash_distance)
                            .then_with(|| metrics.dhash_distance.cmp(&old_metrics.dhash_distance))
                            .then_with(|| metrics.color_distance.total_cmp(&old_metrics.color_distance))
                            .then_with(|| quality_order(units[candidate].representative, units[*old].representative, records)).is_lt(),
                    };
                    if prefer { bridge = Some((candidate, metrics.clone())); }
                }
            }
        }
        if unseen.is_empty() {
            let Some(bridge) = bridge else { continue; };
            unseen.push(bridge);
        }
        unseen.sort_unstable_by(|(a, _), (b, _)| quality_order(units[*a].representative, units[*b].representative, records));
        let mut members = vec![DuplicateMember { record_index: reference, metrics: None }];
        // Include all files in each content unit; they are copies, not additional comparisons.
        for &copy in &units[anchor].members {
            if copy != reference { members.push(DuplicateMember { record_index: copy, metrics: Some(compare_near(near, near, threshold).unwrap()) }); }
        }
        for (candidate, metrics) in &unseen {
            if covered[*candidate] {
                // A bridge only needs the previously displayed reference. Repeating a large
                // exact-copy unit for every bridge would create quadratic output storage.
                members.push(DuplicateMember { record_index: units[*candidate].representative, metrics: Some(metrics.clone()) });
            } else {
                for &record_index in &units[*candidate].members { members.push(DuplicateMember { record_index, metrics: Some(metrics.clone()) }); }
            }
        }
        on_group(DuplicateGroup { kind: DuplicateKind::Near, reference, members })?;
        summary.group_count += 1;
        covered[anchor] = true;
        for (candidate, _) in unseen { covered[candidate] = true; }
    }
    Ok(summary)
}

/// Convenience collector for small fixtures. Production must use build_groups_each.
#[cfg(test)]
pub fn build_groups(records: &[IndexRecord], threshold: Threshold, checkpoint: &mut dyn FnMut() -> Result<(), String>) -> Result<GroupingResult, String> {
    let mut groups = Vec::new();
    let summary = build_groups_each(records, threshold, checkpoint, &mut |group| { groups.push(group); Ok(()) })?;
    Ok(GroupingResult { groups, too_dense_references: summary.too_dense_references, low_information_skipped: summary.low_information_skipped, estimated_memory_bytes: summary.estimated_memory_bytes })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn near(phash: u64) -> NearFingerprint { NearFingerprint { phash, dhash: 42, color: [128; 48], width: 100, height: 100, low_information: false } }
    fn record(index: u32, phash: u64) -> IndexRecord {
        let mut sha256 = [0; 32]; sha256[..4].copy_from_slice(&index.to_le_bytes());
        IndexRecord { image_pk: i64::from(index), image_order: index, file_size: 123, width: 100, height: 100, sha256, near: Some(near(phash)) }
    }
    fn random(state: &mut u64) -> u64 { *state ^= *state << 13; *state ^= *state >> 7; *state ^= *state << 17; *state }

    #[test]
    fn mih_never_misses_distances_within_all_three_thresholds() {
        let mut state = 0x913e_2189_bffa_7701;
        let mut hashes = Vec::new();
        let mut pairs = Vec::new();
        for _ in 0..250 {
            let base = random(&mut state); let first = hashes.len(); hashes.push(base);
            for distance in 1..=6 {
                let mut mask = 0_u64;
                while mask.count_ones() < distance { mask |= 1_u64 << (random(&mut state) % 64); }
                pairs.push((first, hashes.len())); hashes.push(base ^ mask);
            }
        }
        let mut index = MihIndex::new(&hashes, &mut || Ok(())).unwrap();
        for (a, b) in pairs {
            let candidates = index.candidates(hashes[a], &mut || Ok(())).unwrap().unwrap();
            assert!(candidates.contains(&(b as u32)), "missed distance {}", (hashes[a] ^ hashes[b]).count_ones());
        }
    }

    #[test]
    fn group_is_a_star_and_bridge_image_is_not_lost() {
        // A-B and B-C match strict distance 2, A-C does not. A is selected first.
        let records = vec![record(0, 0), record(1, 3), record(2, 15)];
        let result = build_groups(&records, Threshold::Strict, &mut || Ok(())).unwrap();
        assert_eq!(result.groups.len(), 2);
        assert_eq!(result.groups[0].reference, 0);
        assert_eq!(result.groups[0].members.iter().map(|m| m.record_index).collect::<Vec<_>>(), vec![0, 1]);
        assert_eq!(result.groups[1].reference, 2);
        assert_eq!(result.groups[1].members.iter().map(|m| m.record_index).collect::<Vec<_>>(), vec![2, 1]);
        for group in result.groups {
            for member in group.members.iter().skip(1) {
                assert!(compare_near(records[group.reference].near.as_ref().unwrap(), records[member.record_index].near.as_ref().unwrap(), Threshold::Strict).is_some());
            }
        }
    }

    #[test]
    fn low_information_still_forms_exact_groups_but_not_near_groups() {
        let mut records = vec![record(0, 0), record(1, 0), record(2, 0)];
        records[1].sha256 = records[0].sha256;
        for record in &mut records { record.near.as_mut().unwrap().low_information = true; }
        let result = build_groups(&records, Threshold::Loose, &mut || Ok(())).unwrap();
        assert_eq!(result.low_information_skipped, 3);
        assert_eq!(result.groups.len(), 1); assert_eq!(result.groups[0].kind, DuplicateKind::Exact);
        assert_eq!(result.groups[0].members.len(), 2);
    }

    #[test]
    fn copies_are_content_units_and_full_equal_features_do_not_consume_candidate_budget() {
        let mut records: Vec<_> = (0..50_002).map(|i| record(i, 0xabcd)).collect();
        records[1].sha256 = records[0].sha256;
        let result = build_groups(&records, Threshold::Standard, &mut || Ok(())).unwrap();
        assert_eq!(result.groups[0].kind, DuplicateKind::Exact);
        assert_eq!(result.groups[1].kind, DuplicateKind::Near);
        assert_eq!(result.groups[1].members.len(), records.len());
        assert!(result.too_dense_references.is_empty());
    }

    #[test]
    fn auxiliary_filters_do_not_return_different_compositions_or_colors() {
        let a = near(0); let mut b = a.clone(); b.width = 140;
        assert!(compare_near(&a, &b, Threshold::Loose).is_none());
        b = a.clone(); b.dhash = !a.dhash; assert!(compare_near(&a, &b, Threshold::Loose).is_none());
        b = a.clone(); b.color = [200; 48]; assert!(compare_near(&a, &b, Threshold::Loose).is_none());
        b = a.clone(); b.phash = 63; assert!(compare_near(&a, &b, Threshold::Standard).is_none());
        assert!(compare_near(&a, &b, Threshold::Loose).is_some());
    }

    #[test]
    fn density_is_an_explicit_incomplete_result() {
        let hashes = vec![123_u64; MAX_DISTINCT_CANDIDATES + 1];
        let mut index = MihIndex::new(&hashes, &mut || Ok(())).unwrap();
        assert!(index.candidates(123, &mut || Ok(())).unwrap().is_none());
        let mut records: Vec<_> = (0..50_001).map(|i| record(i, 123)).collect();
        // Distinct auxiliary hashes create genuinely distinct complete fingerprints in one pHash bucket.
        for (i, record) in records.iter_mut().enumerate() { record.near.as_mut().unwrap().dhash = i as u64; }
        let result = build_groups(&records, Threshold::Loose, &mut || Ok(())).unwrap();
        assert!(result.groups.is_empty()); assert_eq!(result.too_dense_references.len(), records.len());
    }

    #[test]
    fn cancelled_grouping_does_not_claim_completion_and_large_estimate_is_guarded() {
        let records = vec![record(0, 0)];
        assert_eq!(build_groups(&records, Threshold::Standard, &mut || Err("cancelled".into())).unwrap_err(), "cancelled");
        assert!(estimate_memory_bytes(800_000) < MEMORY_BUDGET_BYTES);
        assert!(estimate_memory_bytes(1_500_000) > MEMORY_BUDGET_BYTES);
    }

    #[test]
    #[ignore = "explicit 800k synthetic scale acceptance; no production database or model"]
    fn eight_hundred_thousand_fingerprints_fit_index_budget() {
        let started = std::time::Instant::now();
        let mut state = 0x913e_2189_bffa_7701;
        let hashes: Vec<_> = (0..800_000).map(|_| random(&mut state)).collect();
        let mut index = MihIndex::new(&hashes, &mut || Ok(())).unwrap();
        assert_eq!(index.postings.len(), 3_200_000);
        let actual = (hashes.capacity() * std::mem::size_of::<u64>() + index.postings.capacity() * std::mem::size_of::<u32>() + index.offsets.capacity() * std::mem::size_of::<u32>() + index.stamps.capacity() * std::mem::size_of::<u32>()) as u64;
        assert!(actual < 32 * 1024 * 1024, "actual numeric MIH storage: {actual}");
        assert!(estimate_memory_bytes(800_000) < MEMORY_BUDGET_BYTES);
        for &position in &[0, 173, 12_456, 421_100, 799_999] {
            let candidates = index.candidates(hashes[position], &mut || Ok(())).unwrap().unwrap();
            assert!(candidates.contains(&(position as u32)));
        }
        eprintln!("[duplicate-mih-scale] count={} actual_numeric_bytes={} grouping_estimate_bytes={} elapsed_ms={}", hashes.len(), actual, estimate_memory_bytes(hashes.len()), started.elapsed().as_millis());
    }
}
