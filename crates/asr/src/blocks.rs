//! Decode fixed PCM blocks so capture packet timing cannot change input leveling.
const BLOCK_SAMPLES: u64 = 3200;

pub fn read_count(
    processed: u64,
    available: u64,
    next_boundary: Option<u64>,
    draining: bool,
) -> Option<u64> {
    let target = (BLOCK_SAMPLES - processed % BLOCK_SAMPLES)
        .min(next_boundary.map_or(u64::MAX, |end| end.saturating_sub(processed)));
    if target == 0 || available == 0 || available < target && !draining {
        None
    } else {
        Some(available.min(target))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_reads_wait_but_pause_eof_and_take_tails_are_never_dropped() {
        assert_eq!(read_count(0, 800, None, false), None);
        assert_eq!(read_count(0, 6400, None, false), Some(3200));
        assert_eq!(read_count(3200, 799, None, true), Some(799));
        assert_eq!(read_count(3200, 799, Some(3999), false), Some(799));
        assert_eq!(read_count(3999, 2401, None, false), Some(2401));
        assert_eq!(read_count(3999, 0, None, true), None);
        assert_eq!(read_count(3999, 1000, Some(3999), false), None);
        // A resumed take may start off-grid; it must still reach the 30s file edge.
        assert_eq!(read_count(479999, 1, None, false), Some(1));
    }

    #[test]
    fn packet_timing_does_not_change_blocks_or_leveled_samples() {
        fn simulate(packets: &[usize]) -> (Vec<u64>, Vec<f32>) {
            let samples: Vec<_> = (0..10417)
                .map(|i| ((i as f32 * 0.013).sin() * 0.002) + if i < 1700 { 0.003 } else { 0. })
                .collect();
            let mut processed = 0;
            let mut delivered = 0;
            let mut packet = 0;
            let mut blocks = vec![];
            let mut output = vec![];
            let mut level = crate::level::SpeechLevel::default();
            while processed < samples.len() {
                delivered = (delivered + packets[packet % packets.len()]).min(samples.len());
                packet += 1;
                while let Some(count) = read_count(
                    processed as u64,
                    (delivered - processed) as u64,
                    None,
                    delivered == samples.len(),
                ) {
                    let end = processed + count as usize;
                    let mut block = samples[processed..end].to_vec();
                    level.process(&mut block);
                    output.extend(block);
                    blocks.push(count);
                    processed = end;
                }
            }
            (blocks, output)
        }
        let reference = simulate(&[10417]);
        assert_eq!(reference.0, vec![3200, 3200, 3200, 817]);
        assert_eq!(reference, simulate(&[1, 17, 512, 47, 2048]));
        assert_eq!(reference, simulate(&[1600, 1200, 3199, 33]));
    }
}
