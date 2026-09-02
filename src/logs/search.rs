use std::collections::BinaryHeap;

use super::Channel;

pub const MAX_RESULTS: usize = 10;

/// Twitch logins are at most 25 characters, so nothing longer can match
/// anything; the cap also bounds the per-entry scoring work below.
pub const MAX_QUERY_LEN: usize = 25;

/// Bit reserved for any byte outside `[a-z0-9_]`. Every such byte aliases
/// onto it, which can only ever produce a false *positive* in the mask
/// prefilter (rejected a moment later by the real match), never a false
/// negative.
const OTHER_BIT: u64 = 1 << 63;

/// `[a-z0-9_]` needs 37 of the mask's bits and `OTHER_BIT` takes the top one,
/// which leaves room to carry the login's length in the same word. The scan
/// needs both for every entry, and folding them together halves the memory
/// it streams.
const LEN_SHIFT: u32 = 37;
const LEN_LIMIT: usize = 63;

/// Entries per block in the scan's rejection pass.
const BLOCK: usize = 32;

fn byte_bit(byte: u8) -> u64 {
    match byte {
        b'a'..=b'z' => 1 << (byte - b'a'),
        b'0'..=b'9' => 1 << (26 + (byte - b'0')),
        b'_' => 1 << 36,
        _ => OTHER_BIT,
    }
}

fn mask_of(value: &[u8]) -> u64 {
    value.iter().fold(0, |mask, &byte| mask | byte_bit(byte))
}

fn word_of(login: &[u8]) -> u64 {
    mask_of(login) | ((login.len().min(LEN_LIMIT) as u64) << LEN_SHIFT)
}

/// The login's length, saturated at `LEN_LIMIT`. Saturating only ever
/// understates it, which keeps the scan's pruning bound a genuine bound.
fn word_len(word: u64) -> usize {
    ((word >> LEN_SHIFT) & LEN_LIMIT as u64) as usize
}

/// How well a login matches a query, ordered best-first by `Ord` (field
/// order is the tie-break order).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct Score {
    /// 0 exact, 1 prefix, 2 matched somewhere inside the login.
    rank: u8,
    /// Characters the match spans — equal to the query length when the
    /// query appears contiguously, larger the more scattered it is.
    span: u32,
    /// Where the match starts; earlier reads as more relevant.
    offset: u32,
    /// Shorter logins win an otherwise equal match.
    len: u32,
}

/// The login's first 8 bytes as a big-endian integer, zero-padded, so that
/// comparing keys orders the same way comparing the strings does.
fn prefix_key(login: &str) -> u64 {
    let mut key = [0u8; 8];
    let bytes = login.as_bytes();
    let take = bytes.len().min(8);
    key[..take].copy_from_slice(&bytes[..take]);
    u64::from_be_bytes(key)
}

/// A query compiled into the shift-and (bitap) tables used to score logins.
///
/// One bit per query character, walked without a data-dependent branch: a
/// permissive query has the scan scoring hundreds of thousands of logins,
/// and a byte-comparison loop mispredicts on nearly every one of them.
struct Matcher<'a> {
    query: &'a [u8],
    /// Bit `i` set for the byte equal to `query[i]`.
    positions: [u64; 256],
    /// The bit that means the whole query has matched.
    goal: u64,
}

impl<'a> Matcher<'a> {
    fn new(query: &'a [u8]) -> Matcher<'a> {
        let mut positions = [0u64; 256];
        for (index, &byte) in query.iter().enumerate() {
            positions[byte as usize] |= 1 << index;
        }

        Matcher {
            query,
            positions,
            goal: 1 << (query.len() - 1),
        }
    }

    /// Scores `login`, or `None` if the query's characters don't appear in
    /// order.
    ///
    /// The match found is the earliest one: it starts at the first character
    /// equal to `query[0]` and completes at the first position where the
    /// whole query has been seen in order. A login that could match more
    /// tightly further along ("ab" in "axbab") is therefore scored on its
    /// earliest match rather than its best one -- the usual fuzzy-finder
    /// approximation, and the only one that stays a single pass.
    fn score(&self, login: &[u8]) -> Option<Score> {
        let len = login.len() as u32;

        if login == self.query {
            return Some(Score {
                rank: 0,
                span: 0,
                offset: 0,
                len,
            });
        }

        let mut state = 0u64;
        let mut offset = usize::MAX;

        for (index, &byte) in login.iter().enumerate() {
            // Sticky bits: bit `i` stays set once `query[..=i]` has been
            // seen, which is what makes this a subsequence match rather
            // than shift-and's contiguous one.
            state |= ((state << 1) | 1) & self.positions[byte as usize];

            if state & 1 != 0 {
                offset = offset.min(index);
            }

            if state & self.goal != 0 {
                let end = index + 1;
                let rank = if offset == 0 && end == self.query.len() {
                    1
                } else {
                    2
                };

                return Some(Score {
                    rank,
                    span: (end - offset) as u32,
                    offset: offset as u32,
                    len,
                });
            }
        }

        None
    }
}

/// The channel set arranged for fuzzy login search.
///
/// Logins and user ids live in two flat buffers with `u32` offsets rather
/// than as a `Vec<Channel>`: at ~1M unique channels a vector of two `String`s
/// each costs well over 100 MB in struct and allocator overhead alone, while
/// this is ~35 MB total and scans as contiguous memory.
///
/// Entries are sorted by login, which gives exact and prefix matches a
/// binary search and — because sorted position *is* alphabetical order —
/// lets equally-scored results tie-break on the entry index without touching
/// the strings.
///
/// `words` holds, per entry, a character-presence bitmap and the login's
/// length. A login can only contain the query as a subsequence if it
/// contains every one of the query's characters, so one `AND` rejects nearly
/// everything a fuzzy query doesn't match; the length then bounds the best
/// score the entry could reach. Only what survives both is scored, and a
/// full scan touches nothing but this one array.
pub struct SearchIndex {
    logins: Box<str>,
    login_offsets: Box<[u32]>,
    ids: Box<str>,
    id_offsets: Box<[u32]>,
    words: Box<[u64]>,
}

impl SearchIndex {
    pub fn empty() -> SearchIndex {
        SearchIndex {
            logins: String::new().into_boxed_str(),
            login_offsets: vec![0].into_boxed_slice(),
            ids: String::new().into_boxed_str(),
            id_offsets: vec![0].into_boxed_slice(),
            words: Vec::new().into_boxed_slice(),
        }
    }

    pub fn len(&self) -> usize {
        self.words.len()
    }

    /// Heap bytes held by the index, for the reload log — this process has
    /// been over-resident before, and the index is the one structure here
    /// that scales with the whole channel set.
    pub fn footprint_bytes(&self) -> usize {
        self.logins.len()
            + self.ids.len()
            + size_of_val(&*self.login_offsets)
            + size_of_val(&*self.id_offsets)
            + size_of_val(&*self.words)
    }

    fn login_str(&self, index: usize) -> &str {
        let start = self.login_offsets[index] as usize;
        let end = self.login_offsets[index + 1] as usize;
        &self.logins[start..end]
    }

    fn login(&self, index: usize) -> &[u8] {
        self.login_str(index).as_bytes()
    }

    fn channel(&self, index: usize) -> Channel {
        let id_start = self.id_offsets[index] as usize;
        let id_end = self.id_offsets[index + 1] as usize;
        Channel {
            name: self.login_str(index).to_string(),
            user_id: self.ids[id_start..id_end].to_string(),
        }
    }

    /// The range of entries whose login starts with `prefix`, found by
    /// binary searching both ends.
    fn prefix_range(&self, prefix: &[u8]) -> std::ops::Range<usize> {
        let bound = |past_prefix: bool| {
            let mut low = 0usize;
            let mut high = self.len();
            while low < high {
                let mid = low + (high - low) / 2;
                let login = self.login(mid);
                let before = if login.len() >= prefix.len() && login.starts_with(prefix) {
                    past_prefix
                } else {
                    login < prefix
                };
                if before {
                    low = mid + 1;
                } else {
                    high = mid;
                }
            }
            low
        };

        bound(false)..bound(true)
    }

    /// The best `limit` channels for an already-normalized, non-empty
    /// `query`, best match first.
    pub fn search(&self, query: &str, limit: usize) -> Vec<Channel> {
        // `Matcher` carries one bit per query character in a `u64`, and
        // nothing longer than a login can match a login anyway.
        if limit == 0 || self.len() == 0 || query.is_empty() || query.len() > MAX_QUERY_LEN {
            return Vec::new();
        }

        let query = query.as_bytes();

        // Every login that starts with the query is an exact or prefix
        // match, and so outranks every entry that merely contains it. Once
        // there are `limit` of them the rest of the set cannot reach the
        // results at all and the scan is pure waste — which is the case
        // short, common queries hit, i.e. the ones with the most entries to
        // reject.
        let prefixed = self.prefix_range(query);
        if prefixed.len() >= limit {
            return self.rank_prefixed(prefixed, limit);
        }

        self.scan(query, prefixed, limit)
    }

    /// Ranks a range that is entirely exact/prefix matches.
    ///
    /// Every entry there scores `(1, query_len, 0, len)` except one of
    /// exactly `query_len` bytes, which is the exact match and sorts first
    /// by length anyway — so length, then index for the alphabetical
    /// tie-break, is the whole ordering, and no login has to be walked.
    fn rank_prefixed(&self, range: std::ops::Range<usize>, limit: usize) -> Vec<Channel> {
        let mut best: BinaryHeap<(usize, usize)> = BinaryHeap::with_capacity(limit + 1);

        for index in range {
            let len = word_len(self.words[index]);
            if best.len() == limit
                && let Some(&worst) = best.peek()
                && (len, index) >= worst
            {
                continue;
            }

            best.push((len, index));
            if best.len() > limit {
                best.pop();
            }
        }

        self.collect(best.into_sorted_vec().into_iter().map(|(_, index)| index))
    }

    /// Full scan for queries with too few prefix matches to fill the
    /// results.
    ///
    /// `prefixed` is the (short, by definition) run of exact/prefix matches;
    /// it seeds the results, and the scan then covers everything either side
    /// of it. Keeping it out of the scan is what makes the pruning bound
    /// below tight: an entry the scan sees cannot be an exact or prefix
    /// match, so the best it can possibly score is `(2, query, 1, len)`.
    fn scan(&self, query: &[u8], prefixed: std::ops::Range<usize>, limit: usize) -> Vec<Channel> {
        let matcher = Matcher::new(query);
        // Max-heap holding the best `limit` so far, so the one to drop is
        // the one on top. Entries are sorted by login, so the index doubles
        // as the alphabetical tie-break and nothing is copied until the end.
        let mut best: BinaryHeap<(Score, usize)> = BinaryHeap::with_capacity(limit + 1);

        for index in prefixed.clone() {
            if let Some(score) = matcher.score(self.login(index)) {
                best.push((score, index));
            }
        }

        let mask = mask_of(query);
        let bound = |len: usize| Score {
            rank: 2,
            span: query.len() as u32,
            offset: 1,
            len: len as u32,
        };

        for segment in [0..prefixed.start, prefixed.end..self.len()] {
            let words = &self.words[segment.clone()];

            // The rejection test runs over fixed-size blocks with the result
            // OR-ed together, which keeps it branch-free and lets the
            // compiler vectorize it. For a typical query this is ~1M entries
            // of pure rejection, and leaving the test at the top of the main
            // loop body instead — where it can't vectorize — costs ~2x.
            for (block, chunk) in words.chunks(BLOCK).enumerate() {
                let mut hit = false;
                for &word in chunk {
                    hit |= word & mask == mask;
                }
                if !hit {
                    continue;
                }

                for (slot, &word) in chunk.iter().enumerate() {
                    if word & mask != mask {
                        continue;
                    }

                    let index = segment.start + block * BLOCK + slot;
                    let len = word_len(word);

                    if len < query.len() {
                        continue;
                    }

                    // Nothing this entry could score would displace the
                    // worst result kept, so its login never has to be read.
                    if best.len() == limit
                        && let Some(worst) = best.peek()
                        && (bound(len), index) >= (worst.0, worst.1)
                    {
                        continue;
                    }

                    let Some(score) = matcher.score(self.login(index)) else {
                        continue;
                    };

                    if best.len() == limit
                        && let Some(worst) = best.peek()
                        && (score, index) >= (worst.0, worst.1)
                    {
                        continue;
                    }

                    best.push((score, index));
                    if best.len() > limit {
                        best.pop();
                    }
                }
            }
        }

        self.collect(best.into_sorted_vec().into_iter().map(|(_, index)| index))
    }

    fn collect(&self, indices: impl Iterator<Item = usize>) -> Vec<Channel> {
        indices.map(|index| self.channel(index)).collect()
    }
}

/// Accumulates entries for a `SearchIndex`.
///
/// Entries are copied into the flat buffers as they arrive, so a caller
/// iterating a `DashMap` can feed borrowed strings one at a time — it never
/// has to materialize an owned copy of the whole channel set first, which at
/// ~1M channels is a >100 MB transient allocation.
pub struct SearchIndexBuilder {
    logins: String,
    login_offsets: Vec<u32>,
    ids: String,
    id_offsets: Vec<u32>,
}

impl SearchIndexBuilder {
    pub fn with_capacity(entries: usize) -> SearchIndexBuilder {
        let mut login_offsets = Vec::with_capacity(entries + 1);
        let mut id_offsets = Vec::with_capacity(entries + 1);
        login_offsets.push(0);
        id_offsets.push(0);

        SearchIndexBuilder {
            logins: String::with_capacity(entries * 16),
            login_offsets,
            ids: String::with_capacity(entries * 10),
            id_offsets,
        }
    }

    pub fn push(&mut self, login: &str, id: &str) {
        // `u32` offsets cap the buffers at 4 GB. A channel set that large is
        // not a thing that happens, but dropping the overflow is better than
        // wrapping an offset into nonsense.
        if self.logins.len() + login.len() > u32::MAX as usize
            || self.ids.len() + id.len() > u32::MAX as usize
        {
            return;
        }

        self.logins.push_str(login);
        self.ids.push_str(id);
        self.login_offsets.push(self.logins.len() as u32);
        self.id_offsets.push(self.ids.len() as u32);
    }

    /// Sorts by login and lays the entries out in that order, which is what
    /// makes prefix lookup a binary search and lets the entry index stand in
    /// for the alphabetical tie-break.
    pub fn finish(self) -> SearchIndex {
        let SearchIndexBuilder {
            logins,
            login_offsets,
            ids,
            id_offsets,
        } = self;

        let count = login_offsets.len() - 1;
        let login_at = |index: usize| -> &str {
            &logins[login_offsets[index] as usize..login_offsets[index + 1] as usize]
        };

        // Sorting on the login's first 8 bytes packed into an integer,
        // with the string compare only as a tie-break, keeps ~1M
        // comparisons from each chasing two offsets into the login buffer.
        // Zero padding is correct: it sorts a shorter login before any
        // longer one sharing its prefix, which is the string order.
        let mut order: Vec<(u64, u32)> = (0..count as u32)
            .map(|index| (prefix_key(login_at(index as usize)), index))
            .collect();
        order.sort_unstable_by(|&(a_key, a), &(b_key, b)| {
            a_key
                .cmp(&b_key)
                .then_with(|| login_at(a as usize).cmp(login_at(b as usize)))
        });

        let mut sorted_logins = String::with_capacity(logins.len());
        let mut sorted_login_offsets: Vec<u32> = Vec::with_capacity(count + 1);
        let mut sorted_ids = String::with_capacity(ids.len());
        let mut sorted_id_offsets: Vec<u32> = Vec::with_capacity(count + 1);
        let mut words: Vec<u64> = Vec::with_capacity(count);
        sorted_login_offsets.push(0);
        sorted_id_offsets.push(0);

        for &(_, index) in &order {
            let index = index as usize;
            let login = login_at(index);
            words.push(word_of(login.as_bytes()));
            sorted_logins.push_str(login);
            sorted_login_offsets.push(sorted_logins.len() as u32);
            sorted_ids.push_str(&ids[id_offsets[index] as usize..id_offsets[index + 1] as usize]);
            sorted_id_offsets.push(sorted_ids.len() as u32);
        }

        SearchIndex {
            logins: sorted_logins.into_boxed_str(),
            login_offsets: sorted_login_offsets.into_boxed_slice(),
            ids: sorted_ids.into_boxed_str(),
            id_offsets: sorted_id_offsets.into_boxed_slice(),
            words: words.into_boxed_slice(),
        }
    }
}
