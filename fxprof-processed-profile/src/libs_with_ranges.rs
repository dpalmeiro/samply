#[derive(Debug, Clone)]
pub struct LibMappings<T> {
    sorted_lib_ranges: Vec<Mapping<T>>,
}

impl<T> Default for LibMappings<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> LibMappings<T> {
    pub fn new() -> Self {
        Self {
            sorted_lib_ranges: Vec::new(),
        }
    }

    /// Add a mapping to this process.
    ///
    /// `start_avma..end_avma` describe the address range that this mapping
    /// occupies in the virtual memory address space of the process.
    /// AVMA = "actual virtual memory address"
    ///
    /// `relative_address_at_start` is the "relative address" which corresponds
    /// to `start_avma`, in the library that is mapped in this mapping. A relative
    /// address is a `u32` value which is relative to the library base address.
    /// So you will usually set `relative_address_at_start` to `start_avma - base_avma`.
    ///
    /// For ELF binaries, the base address is AVMA of the first segment, i.e. the
    /// start_avma of the mapping created by the first ELF `LOAD` command.
    ///
    /// For mach-O binaries, the base address is the vmaddr of the `__TEXT` segment.
    ///
    /// For Windows binaries, the base address is the image load address.
    pub fn add_mapping(
        &mut self,
        start_avma: u64,
        end_avma: u64,
        relative_address_at_start: u32,
        value: T,
    ) {
        let insertion_index = match self
            .sorted_lib_ranges
            .binary_search_by_key(&start_avma, |r| r.start_avma)
        {
            Ok(i) => {
                // We already have a library mapping at this address.
                // Not sure how to best deal with it. Ideally it wouldn't happen. Let's just remove this mapping.
                self.sorted_lib_ranges.remove(i);
                i
            }
            Err(i) => i,
        };

        self.sorted_lib_ranges.insert(
            insertion_index,
            Mapping {
                start_avma,
                end_avma,
                relative_address_at_start,
                value,
            },
        );
    }

    pub fn remove_mapping(&mut self, start_avma: u64) {
        self.sorted_lib_ranges
            .retain(|r| r.start_avma != start_avma);
    }

    fn lookup(&self, avma: u64) -> Option<&Mapping<T>> {
        let ranges = &self.sorted_lib_ranges[..];
        let index = match ranges.binary_search_by_key(&avma, |r| r.start_avma) {
            Err(0) => return None,
            Ok(exact_match) => exact_match,
            Err(insertion_index) => {
                let range_index = insertion_index - 1;
                if avma < ranges[range_index].end_avma {
                    range_index
                } else {
                    return None;
                }
            }
        };
        Some(&ranges[index])
    }

    /// Converts an absolute address (AVMA, actual virtual memory address) into
    /// a relative address and the mapping's associated value.
    pub fn convert_address(&self, avma: u64) -> Option<(u32, &T)> {
        let range = match self.lookup(avma) {
            Some(range) => range,
            None => return None,
        };
        let offset_from_mapping_start = (avma - range.start_avma) as u32;
        let relative_address = range.relative_address_at_start + offset_from_mapping_start;
        Some((relative_address, &range.value))
    }
}

#[derive(Debug, Clone, PartialEq, PartialOrd, Ord, Eq)]
struct Mapping<T> {
    start_avma: u64,
    end_avma: u64,
    relative_address_at_start: u32,
    value: T,
}
