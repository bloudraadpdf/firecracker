// SPDX-License-Identifier: Apache-2.0

//! ACPI timer and serial tables for Arm firmware boot.

use vm_memory::{Bytes, GuestAddress, GuestMemory};
use zerocopy::IntoBytes;

use crate::{Result, Sdt, SdtHeader, checksum};

#[derive(Debug)]
pub struct ArmTable(Vec<u8>);

impl ArmTable {
    fn new(signature: [u8; 4], revision: u8, body: &[u8]) -> Self {
        let mut header = SdtHeader::new(
            signature,
            u32::try_from(size_of::<SdtHeader>() + body.len()).unwrap(),
            revision,
            *b"FIRECK",
            *b"FCVMARM ",
            0,
        );
        header.checksum = checksum(&[header.as_bytes(), body]);
        let mut bytes = header.as_bytes().to_vec();
        bytes.extend_from_slice(body);
        Self(bytes)
    }

    /// Generic Timer Description Table for the architectural system-register timers.
    pub fn timer() -> Self {
        let mut body = [0; 68];
        body[..8].copy_from_slice(&u64::MAX.to_le_bytes());
        for (offset, interrupt, flags) in
            [(12, 29_u32, 0_u32), (20, 30, 4), (28, 27, 0), (36, 26, 0)]
        {
            body[offset..offset + 4].copy_from_slice(&interrupt.to_le_bytes());
            body[offset + 4..offset + 8].copy_from_slice(&flags.to_le_bytes());
        }
        body[44..52].copy_from_slice(&u64::MAX.to_le_bytes());
        Self::new(*b"GTDT", 2, &body)
    }

    /// Serial Port Console Redirection Table for a byte-addressed MMIO 16550 UART.
    pub fn serial(address: u64, interrupt: u32) -> Self {
        let mut body = [0; 44];
        body[5] = 8;
        body[7] = 1;
        body[8..16].copy_from_slice(&address.to_le_bytes());
        body[16] = 1 << 3;
        body[18..22].copy_from_slice(&interrupt.to_le_bytes());
        body[22] = 7;
        body[24] = 1;
        body[28..32].fill(0xff);
        Self::new(*b"SPCR", 2, &body)
    }

    /// Map bus zero's PCI requester IDs to GIC ITS zero without an IOMMU.
    pub fn io_remapping() -> Self {
        let mut body = [0; 100];
        body[..4].copy_from_slice(&2_u32.to_le_bytes());
        body[4..8].copy_from_slice(&48_u32.to_le_bytes());
        let its = &mut body[12..36];
        its[1..3].copy_from_slice(&24_u16.to_le_bytes());
        its[3] = 1;
        its[16..20].copy_from_slice(&1_u32.to_le_bytes());
        let root = &mut body[36..];
        root[0] = 2;
        root[1..3].copy_from_slice(&64_u16.to_le_bytes());
        root[3] = 4;
        root[4..8].copy_from_slice(&1_u32.to_le_bytes());
        root[8..12].copy_from_slice(&1_u32.to_le_bytes());
        root[12..16].copy_from_slice(&40_u32.to_le_bytes());
        root[16..20].copy_from_slice(&1_u32.to_le_bytes());
        root[32] = 40;
        root[44..48].copy_from_slice(&255_u32.to_le_bytes());
        root[52..56].copy_from_slice(&48_u32.to_le_bytes());
        Self::new(*b"IORT", 6, &body)
    }
}

/// Interrupt controller addresses for a GICv3 platform with one ITS.
#[derive(Debug)]
pub struct GicV3Topology {
    pub distributor: u64,
    pub redistributors: u64,
    pub redistributors_size: u32,
    pub its: u64,
    pub pmu_irq: Option<u32>,
}

impl GicV3Topology {
    /// Encode MADT records in the same CPU order as the platform's vCPUs.
    pub fn madt_records(&self, mpidrs: &[u64]) -> Vec<u8> {
        let mut records = Vec::new();
        for (index, mpidr) in mpidrs.iter().enumerate() {
            let mut cpu = [0; 80];
            cpu[0] = 11;
            cpu[1] = 80;
            let index = u32::try_from(index).unwrap();
            cpu[4..8].copy_from_slice(&index.to_le_bytes());
            cpu[8..12].copy_from_slice(&index.to_le_bytes());
            cpu[12..16].copy_from_slice(&1_u32.to_le_bytes());
            cpu[20..24].copy_from_slice(&self.pmu_irq.unwrap_or(0).to_le_bytes());
            cpu[68..76].copy_from_slice(&(mpidr & 0x00ff_00ff_ffff).to_le_bytes());
            records.extend_from_slice(&cpu);
        }
        let mut distributor = [0; 24];
        distributor[0] = 12;
        distributor[1] = 24;
        distributor[8..16].copy_from_slice(&self.distributor.to_le_bytes());
        distributor[20] = 3;
        records.extend_from_slice(&distributor);
        let mut redistributors = [0; 16];
        redistributors[0] = 14;
        redistributors[1] = 16;
        redistributors[4..12].copy_from_slice(&self.redistributors.to_le_bytes());
        redistributors[12..16].copy_from_slice(&self.redistributors_size.to_le_bytes());
        records.extend_from_slice(&redistributors);
        let mut its = [0; 20];
        its[0] = 15;
        its[1] = 20;
        its[8..16].copy_from_slice(&self.its.to_le_bytes());
        records.extend_from_slice(&its);
        records
    }
}

impl Sdt for ArmTable {
    fn len(&self) -> usize {
        self.0.len()
    }

    fn write_to_guest<M: GuestMemory>(&mut self, mem: &M, address: GuestAddress) -> Result<()> {
        mem.write_slice(&self.0, address)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gic_cpu_affinities_and_pmu_interrupts() {
        let mut topology = GicV3Topology {
            distributor: 0x3fff_0000,
            redistributors: 0x3ffb_0000,
            redistributors_size: 0x4_0000,
            its: 0x3ff9_0000,
            pmu_irq: None,
        };
        for pmu_irq in [None, Some(23)] {
            topology.pmu_irq = pmu_irq;
            let records = topology.madt_records(&[0x8000_0000, 0x1280_030201]);
            assert_eq!(records.len(), 2 * 80 + 24 + 16 + 20);
            for (index, affinity) in [0_u64, 0x1200_030201].into_iter().enumerate() {
                let cpu = &records[index * 80..(index + 1) * 80];
                assert_eq!(&cpu[..2], &[11, 80]);
                assert_eq!(
                    u32::from_le_bytes(cpu[8..12].try_into().unwrap()),
                    u32::try_from(index).unwrap()
                );
                assert_eq!(
                    u32::from_le_bytes(cpu[20..24].try_into().unwrap()),
                    pmu_irq.unwrap_or(0)
                );
                assert_eq!(
                    u64::from_le_bytes(cpu[68..76].try_into().unwrap()),
                    affinity
                );
            }
            assert_eq!(&records[160..162], &[12, 24]);
            assert_eq!(&records[184..186], &[14, 16]);
            assert_eq!(&records[200..202], &[15, 20]);
        }
    }

    #[test]
    fn architectural_timer_and_uart_tables_have_valid_layouts() {
        let timer = ArmTable::timer();
        let serial = ArmTable::serial(0x4000_2000, 33);
        for (table, signature, length) in [(&timer, b"GTDT", 104), (&serial, b"SPCR", 80)] {
            assert_eq!(&table.0[..4], signature);
            assert_eq!(table.len(), length);
            assert_eq!(
                u32::from_le_bytes(table.0[4..8].try_into().unwrap()) as usize,
                length
            );
            assert_eq!(
                table
                    .0
                    .iter()
                    .fold(0_u8, |sum, byte| sum.wrapping_add(*byte)),
                0
            );
        }
        assert_eq!(&timer.0[36..44], &u64::MAX.to_le_bytes());
        assert_eq!(&timer.0[56..60], &30_u32.to_le_bytes());
        assert_eq!(&timer.0[64..68], &27_u32.to_le_bytes());
        assert_eq!(&serial.0[44..52], &0x4000_2000_u64.to_le_bytes());
        assert_eq!(&serial.0[54..58], &33_u32.to_le_bytes());
        assert_eq!(serial.0[58], 7);
    }
}
