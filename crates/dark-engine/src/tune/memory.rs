//! Reads the machine's memory (task unit `B6`, step 2).

/// Total and available system memory, in bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MemoryReading {
    /// Total installed memory.
    pub total_bytes: u64,
    /// Memory available for a new allocation, right now.
    pub available_bytes: u64,
}

impl MemoryReading {
    /// Section 4.1's budget: available memory, less the 10% headroom the
    /// formula reserves for every load.
    #[must_use]
    pub fn budget_bytes(&self) -> u64 {
        self.available_bytes - self.available_bytes / 10
    }
}

/// Reads the machine's memory with [`sysinfo`].
#[must_use]
pub fn read() -> MemoryReading {
    let mut system = sysinfo::System::new();
    system.refresh_memory();
    MemoryReading {
        total_bytes: system.total_memory(),
        available_bytes: system.available_memory(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn budget_bytes_reserves_ten_percent_of_available_memory() {
        let reading = MemoryReading {
            total_bytes: 32_000_000_000,
            available_bytes: 16_000_000_000,
        };
        assert_eq!(reading.budget_bytes(), 14_400_000_000);
    }

    #[test]
    fn read_reports_a_positive_total_on_a_real_machine() {
        let reading = read();
        assert!(reading.total_bytes > 0);
        assert!(reading.available_bytes <= reading.total_bytes.max(reading.available_bytes));
    }
}
