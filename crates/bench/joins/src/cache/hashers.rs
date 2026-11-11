//! The hash join with hashers that run more instructions, and with keys
//! hashed in batches before their lookups.

use super::*;

/// Rows a side [`time_hashers`] runs at: one size whose table fits in L1's
/// neighbourhood, the flat rate's, and three past L2.
pub const HASHER_SIZES: [usize; 5] = [1 << 14, 1 << 18, 1 << 20, 1 << 22, 1 << 24];

/// One hashing's cost at one size, each the median per input row.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct HasherCost {
    pub rows_a_side: usize,
    /// The [`Hashing`]'s label.
    pub hashing: String,
    pub ns: f64,
    /// Instructions retired, as the kernel counts them.
    pub instructions: f64,
}

/// What the `hashers` target wrote.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct HashersPart {
    pub sentinels: Sentinels,
    pub hashers: Vec<HasherCost>,
}

/// How [`time_hashers`] hashes the join's keys.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hashing {
    /// [`hash_join`] with foldhash, as the main case runs it.
    Fold,
    /// [`hash_join`] with foldhash and 16 more rounds of mixing.
    Padded16,
    /// [`hash_join`] with foldhash and 32 more rounds of mixing.
    Padded32,
    /// [`hash_join`] with std's SipHash-1-3, with fixed keys, as the main
    /// case runs it.
    Sip,
    /// [`batched_hash_join`] with SipHash, 16 keys at a time: too few for
    /// their lookups to fill the window, so the next batch's hashing does.
    SipBatch16,
    /// [`batched_hash_join`] with SipHash, 256 keys at a time.
    SipBatch256,
    /// [`batched_hash_join`] with foldhash, 256 keys at a time: what batching
    /// does to a hash that's already cheap.
    FoldBatch256,
}

/// Every [`Hashing`], in the order the report's columns take.
pub const HASHINGS: [Hashing; 7] = [
    Hashing::Fold,
    Hashing::Padded16,
    Hashing::Padded32,
    Hashing::Sip,
    Hashing::SipBatch16,
    Hashing::SipBatch256,
    Hashing::FoldBatch256,
];

impl Hashing {
    pub fn label(self) -> &'static str {
        match self {
            Hashing::Fold => "foldhash",
            Hashing::Padded16 => "foldhash + 16 rounds",
            Hashing::Padded32 => "foldhash + 32 rounds",
            Hashing::Sip => "SipHash",
            Hashing::SipBatch16 => "SipHash, 16 at a time",
            Hashing::SipBatch256 => "SipHash, 256 at a time",
            Hashing::FoldBatch256 => "foldhash, 256 at a time",
        }
    }

    /// Joins `build` and `probe` this way.
    pub fn join<S: Sink>(self, build: &[Row], probe: &[Row], out: &mut S) {
        let sip = BuildHasherDefault::<DefaultHasher>::default;
        let pair = |l: &Row, r: &Row| out.pair(l, r);
        match self {
            Hashing::Fold => hash_join(build, probe, FixedState::default(), pair),
            Hashing::Padded16 => hash_join(build, probe, Padded::<16>, pair),
            Hashing::Padded32 => hash_join(build, probe, Padded::<32>, pair),
            Hashing::Sip => hash_join(build, probe, sip(), pair),
            Hashing::SipBatch16 => batched_hash_join::<16, _>(build, probe, sip(), out),
            Hashing::SipBatch256 => batched_hash_join::<256, _>(build, probe, sip(), out),
            Hashing::FoldBatch256 => {
                batched_hash_join::<256, _>(build, probe, FixedState::default(), out);
            }
        }
    }
}

/// foldhash, then `ROUNDS` rounds of a multiply and an xorshift: more
/// instructions per key, the same table and the same reads.
#[derive(Clone, Copy, Debug, Default)]
pub struct Padded<const ROUNDS: usize>;

/// [`Padded`]'s hasher.
pub struct PaddedHasher<const ROUNDS: usize>(FoldHasher<'static>);

impl<const ROUNDS: usize> BuildHasher for Padded<ROUNDS> {
    type Hasher = PaddedHasher<ROUNDS>;

    fn build_hasher(&self) -> Self::Hasher {
        static SEED: FixedState = FixedState::with_seed(0);
        PaddedHasher(SEED.build_hasher())
    }
}

impl<const ROUNDS: usize> Hasher for PaddedHasher<ROUNDS> {
    fn write(&mut self, bytes: &[u8]) {
        self.0.write(bytes);
    }

    fn write_u64(&mut self, x: u64) {
        self.0.write_u64(x);
    }

    fn finish(&self) -> u64 {
        let mut hash = self.0.finish();
        for _ in 0..ROUNDS {
            hash = (hash ^ (hash >> 29)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        }
        hash
    }
}

/// [`hash_join`], but hashing `BATCH` keys before it inserts or looks up any
/// of them, as engines do. hashbrown's `HashTable`, the table under std's
/// `HashMap`, takes a hash the caller computed. Each entry holds the same two
/// references `hash_join`'s does, and an insert looks for the key first as
/// `HashMap::insert` does, so the table and its reads are the same.
pub fn batched_hash_join<const BATCH: usize, S: Sink>(
    build: &[Row],
    probe: &[Row],
    hasher: impl BuildHasher,
    out: &mut S,
) {
    let mut table: HashTable<(&u64, &Row)> = HashTable::with_capacity(build.len());
    let rehash = |&(key, _): &(&u64, &Row)| hasher.hash_one(key);
    let mut hashes = [0; BATCH];
    for rows in build.chunks(BATCH) {
        for (hash, row) in hashes.iter_mut().zip(rows) {
            *hash = hasher.hash_one(row.0);
        }
        for (&hash, row) in hashes.iter().zip(rows) {
            match table.entry(hash, |&(key, _)| *key == row.0, rehash) {
                Entry::Occupied(mut held) => *held.get_mut() = (&row.0, row),
                Entry::Vacant(slot) => {
                    slot.insert((&row.0, row));
                }
            }
        }
    }
    for rows in probe.chunks(BATCH) {
        for (hash, row) in hashes.iter_mut().zip(rows) {
            *hash = hasher.hash_one(row.0);
        }
        for (&hash, row) in hashes.iter().zip(rows) {
            if let Some(&(_, found)) = table.find(hash, |&(key, _)| *key == row.0) {
                out.pair(found, row);
            }
        }
    }
}

/// Times and counts the instructions of each of [`HASHINGS`] at
/// `rows_a_side`, on the main case's inputs and its rotation, as
/// [`time_phases`] does.
///
/// # Panics
///
/// If a call emits the wrong number of pairs.
#[inline]
pub fn time_hashers(rows_a_side: usize) -> Vec<HasherCost> {
    let instances = harness::rotation(2 * rows_a_side as u64);
    let inputs: Vec<_> = (0..instances)
        .map(|i| workload::one_to_one(rows_a_side, i))
        .collect();
    let rows = (2 * rows_a_side) as f64;
    HASHINGS
        .iter()
        .map(|&hashing| {
            let mut next = inputs.iter().cycle();
            let mut call = || {
                let input = next.next().expect("a case has an instance");
                let mut tally = Tally::default();
                let counted = instructions();
                let start = Instant::now();
                hashing.join(black_box(&input.left), black_box(&input.right), &mut tally);
                let elapsed = start.elapsed();
                let retired = instructions() - counted;
                assert_eq!(tally.tuples, input.expected, "{hashing:?} at {rows_a_side}");
                (elapsed.as_nanos() as f64 / rows, retired as f64 / rows)
            };
            (0..instances).for_each(|_| {
                call();
            });
            let start = Instant::now();
            let mut samples = Vec::new();
            while samples.len() < harness::SAMPLES || start.elapsed() < harness::MEASUREMENT {
                samples.push(call());
            }
            let (mut ns, mut counts): (Vec<f64>, Vec<f64>) = samples.into_iter().unzip();
            HasherCost {
                rows_a_side,
                hashing: hashing.label().to_owned(),
                ns: median(&mut ns),
                instructions: median(&mut counts),
            }
        })
        .collect()
}
