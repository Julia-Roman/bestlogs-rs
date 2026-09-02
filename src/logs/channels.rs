use serde::{Serialize, Serializer};

use super::Channel;

/// An instance's `/channels` list, arranged so a membership test is a binary
/// search instead of a scan.
///
/// `get_instance` probes every alive instance on every lookup, and the lists
/// are enormous (one instance alone carries ~1M channels, ~1.6M across the
/// configured set), so an O(n) membership test costs ~1.6M string
/// comparisons per request.
///
/// `channels` is held sorted by login, which is what the overwhelming
/// majority of lookups search by — that path is a direct binary search with
/// no indirection. Only an `id:`-style reference needs `by_id`, which costs
/// 4 bytes per channel.
pub struct InstanceChannels {
    channels: Vec<Channel>,
    by_id: Vec<u32>,
}

impl InstanceChannels {
    pub fn new(mut channels: Vec<Channel>) -> InstanceChannels {
        // `u32` indices cap this at 4B channels per instance; the truncation
        // guard keeps a nonsensical upstream response from silently aliasing
        // index 0 rather than just being ignored.
        let len = channels.len().min(u32::MAX as usize);
        channels.sort_unstable_by(|a, b| a.name.cmp(&b.name));
        let mut by_id: Vec<u32> = (0..len as u32).collect();
        by_id.sort_unstable_by(|&a, &b| {
            channels[a as usize]
                .user_id
                .cmp(&channels[b as usize].user_id)
        });
        InstanceChannels { channels, by_id }
    }

    pub fn empty() -> InstanceChannels {
        InstanceChannels {
            channels: Vec::new(),
            by_id: Vec::new(),
        }
    }

    /// Whether any channel here matches `value` as either a login or a
    /// Twitch user id — the same either/or test the linear scan did, since
    /// callers pass an already-`id:`-stripped token that can legitimately be
    /// a numeric login.
    pub fn contains(&self, value: &str) -> bool {
        self.channels
            .binary_search_by(|c| c.name.as_str().cmp(value))
            .is_ok()
            || self
                .by_id
                .binary_search_by(|&i| self.channels[i as usize].user_id.as_str().cmp(value))
                .is_ok()
    }

    pub fn is_empty(&self) -> bool {
        self.channels.is_empty()
    }

    pub fn len(&self) -> usize {
        self.channels.len()
    }
}

/// Serializes as the bare channel array, the same shape `/instances` returned
/// when this was a plain `Vec<Channel>` (in login order rather than the
/// instance's own; no client depends on the ordering).
impl Serialize for InstanceChannels {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.channels.serialize(serializer)
    }
}
