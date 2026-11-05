// Copyright 2024 Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0

use std::mem::offset_of;

use gdbstub_arch::aarch64::reg::AArch64CoreRegs as CoreRegs;
use kvm_bindings::{
    KVM_GUESTDBG_ENABLE, KVM_GUESTDBG_SINGLESTEP, KVM_GUESTDBG_USE_HW, KVM_GUESTDBG_USE_SW_BP,
    KVM_REG_ARM_CORE, KVM_REG_ARM64, KVM_REG_SIZE_U64, kvm_guest_debug, kvm_regs, user_pt_regs,
};
use kvm_ioctls::VcpuFd;
use vm_memory::GuestAddress;

use crate::Vmm;
use crate::arch::aarch64::regs::{Aarch64RegisterVec, arm64_core_reg_id};
use crate::arch::aarch64::vcpu::get_registers;
use crate::gdb::target::GdbTargetError;

/// Configures the number of bytes required for a software breakpoint.
///
/// The breakpoint instruction operation also includes the immediate argument which we 0 hence the
/// size.
pub const SW_BP_SIZE: usize = 4;

/// The bytes stored for a software breakpoint.
///
/// This is the BRK instruction with a 0 immediate argument.
/// https://developer.arm.com/documentation/ddi0602/2024-09/Base-Instructions/BRK--Breakpoint-instruction-
pub const SW_BP: [u8; SW_BP_SIZE] = [0, 0, 32, 212];

/// Register id for the program counter
const PC_REG_ID: u64 = arm64_core_reg_id!(KVM_REG_SIZE_U64, offset_of!(user_pt_regs, pc));

/// Retrieve a single register from a Vcpu
fn get_sys_reg(reg: u64, vcpu_fd: &VcpuFd) -> Result<u64, GdbTargetError> {
    let mut register_vec = Aarch64RegisterVec::default();
    get_registers(vcpu_fd, &[reg], &mut register_vec)?;
    let register = register_vec
        .iter()
        .next()
        .ok_or(GdbTargetError::ReadRegisterVecError)?;

    Ok(register.value())
}

/// Gets the PC value for a Vcpu
pub fn get_instruction_pointer(vcpu_fd: &VcpuFd) -> Result<u64, GdbTargetError> {
    get_sys_reg(PC_REG_ID, vcpu_fd)
}

/// Translates a virtual address according to the Vcpu's current address translation mode.
/// Returns the GPA (guest physical address)
pub fn translate_gva(vcpu_fd: &VcpuFd, gva: u64, vmm: &Vmm) -> Result<u64, GdbTargetError> {
    Ok(crate::arch::aarch64::paging::translate_kernel_address(
        vcpu_fd,
        vmm.vm.as_kvm().expect("GDB requires KVM").guest_memory(),
        gva,
    )?)
}

/// Configures the kvm guest debug regs to register the hardware breakpoints
fn set_kvm_debug(
    control: u32,
    vcpu_fd: &VcpuFd,
    addrs: &[GuestAddress],
) -> Result<(), GdbTargetError> {
    let mut dbg = kvm_guest_debug {
        control,
        ..Default::default()
    };

    for (i, addr) in addrs.iter().enumerate() {
        // DBGBCR_EL1 (Debug Breakpoint Control Registers, D13.3.2):
        // bit 0: 1 (Enabled)
        // bit 1~2: 0b11 (PMC = EL1/EL0)
        // bit 5~8: 0b1111 (BAS = AArch64)
        // others: 0
        dbg.arch.dbg_bcr[i] = 0b1 | (0b11 << 1) | (0b1111 << 5);
        // DBGBVR_EL1 (Debug Breakpoint Value Registers, D13.3.3):
        // bit 2~52: VA[2:52]
        dbg.arch.dbg_bvr[i] = (!0u64 >> 11) & addr.0;
    }

    vcpu_fd.set_guest_debug(&dbg)?;

    Ok(())
}

/// Register id for pstate
const PSTATE_ID: u64 = arm64_core_reg_id!(KVM_REG_SIZE_U64, offset_of!(user_pt_regs, pstate));

/// Configures the Vcpu for debugging and sets the hardware breakpoints on the Vcpu
pub fn vcpu_set_debug(
    vcpu_fd: &VcpuFd,
    addrs: &[GuestAddress],
    step: bool,
) -> Result<(), GdbTargetError> {
    let mut control = KVM_GUESTDBG_ENABLE | KVM_GUESTDBG_USE_HW | KVM_GUESTDBG_USE_SW_BP;
    if step {
        control |= KVM_GUESTDBG_SINGLESTEP;
    }

    set_kvm_debug(control, vcpu_fd, addrs)
}

/// Deliver a guest BRK exception to its EL1 vector.
pub fn vcpu_inject_bp(
    vcpu_fd: &VcpuFd,
    _addrs: &[GuestAddress],
    _step: bool,
    debug: kvm_bindings::kvm_debug_exit_arch,
) -> Result<(), GdbTargetError> {
    use kvm_bindings::{
        PSR_C_BIT, PSR_DIT_BIT, PSR_N_BIT, PSR_PAN_BIT, PSR_SSBS_BIT, PSR_V_BIT, PSR_Z_BIT,
    };

    let pstate = get_sys_reg(PSTATE_ID, vcpu_fd)?;
    let vector_offset = match pstate & 0x1f {
        0 => 0x400,
        4 => 0,
        5 => 0x200,
        _ => return Err(GdbTargetError::UnsupportedGuestDebug(debug.hsr)),
    };
    if debug.hsr >> 26 != 0x3c {
        return Err(GdbTargetError::UnsupportedGuestDebug(debug.hsr));
    }
    let pc = get_instruction_pointer(vcpu_fd)?;
    let vector = get_sys_reg(0x603000000013c600, vcpu_fd)? + vector_offset;
    let sctlr = get_sys_reg(0x603000000013c080, vcpu_fd)?;
    let mut handler_state = pstate
        & u64::from(PSR_N_BIT | PSR_Z_BIT | PSR_C_BIT | PSR_V_BIT | PSR_DIT_BIT | PSR_PAN_BIT);
    handler_state |= 0x3c5;
    if sctlr & (1 << 23) == 0 {
        handler_state |= u64::from(PSR_PAN_BIT);
    }
    if sctlr & (1 << 44) != 0 {
        handler_state |= u64::from(PSR_SSBS_BIT);
    }
    for (reg, value) in [
        (
            arm64_core_reg_id!(KVM_REG_SIZE_U64, offset_of!(kvm_regs, elr_el1)),
            pc,
        ),
        (
            arm64_core_reg_id!(KVM_REG_SIZE_U64, offset_of!(kvm_regs, spsr)),
            pstate,
        ),
        (0x603000000013c290, u64::from(debug.hsr)),
        (PSTATE_ID, handler_state),
        (PC_REG_ID, vector),
    ] {
        vcpu_fd.set_one_reg(reg, &value.to_le_bytes())?;
    }
    Ok(())
}
/// The number of general purpose registers
const GENERAL_PURPOSE_REG_COUNT: usize = 31;
/// The number of core registers we read from the Vcpu
const CORE_REG_COUNT: usize = 33;
/// Stores the register ids of registers to be read from the Vcpu
const CORE_REG_IDS: [u64; CORE_REG_COUNT] = {
    let mut regs = [0; CORE_REG_COUNT];
    let mut idx = 0;

    let reg_offset = offset_of!(kvm_regs, regs);
    let mut off = reg_offset;
    while idx < GENERAL_PURPOSE_REG_COUNT {
        regs[idx] = arm64_core_reg_id!(KVM_REG_SIZE_U64, off);
        idx += 1;
        off += std::mem::size_of::<u64>();
    }

    regs[idx] = arm64_core_reg_id!(KVM_REG_SIZE_U64, offset_of!(user_pt_regs, sp));
    idx += 1;

    regs[idx] = arm64_core_reg_id!(KVM_REG_SIZE_U64, offset_of!(user_pt_regs, pc));
    regs
};

/// Reads the registers for the Vcpu
pub fn read_registers(vcpu_fd: &VcpuFd, regs: &mut CoreRegs) -> Result<(), GdbTargetError> {
    let mut register_vec = Aarch64RegisterVec::default();
    get_registers(vcpu_fd, &CORE_REG_IDS, &mut register_vec)?;

    let mut registers = register_vec.iter();

    for i in 0..GENERAL_PURPOSE_REG_COUNT {
        regs.x[i] = registers
            .next()
            .ok_or(GdbTargetError::ReadRegisterVecError)?
            .value();
    }

    regs.sp = registers
        .next()
        .ok_or(GdbTargetError::ReadRegisterVecError)?
        .value();

    regs.pc = registers
        .next()
        .ok_or(GdbTargetError::ReadRegisterVecError)?
        .value();

    Ok(())
}

/// Writes to the registers for the Vcpu
pub fn write_registers(vcpu_fd: &VcpuFd, regs: &CoreRegs) -> Result<(), GdbTargetError> {
    let kreg_off = offset_of!(kvm_regs, regs);
    let mut off = kreg_off;
    for i in 0..GENERAL_PURPOSE_REG_COUNT {
        vcpu_fd.set_one_reg(
            arm64_core_reg_id!(KVM_REG_SIZE_U64, off),
            &regs.x[i].to_le_bytes(),
        )?;
        off += std::mem::size_of::<u64>();
    }

    let off = offset_of!(user_pt_regs, sp);
    vcpu_fd.set_one_reg(
        arm64_core_reg_id!(KVM_REG_SIZE_U64, off + kreg_off),
        &regs.sp.to_le_bytes(),
    )?;

    let off = offset_of!(user_pt_regs, pc);
    vcpu_fd.set_one_reg(
        arm64_core_reg_id!(KVM_REG_SIZE_U64, off + kreg_off),
        &regs.pc.to_le_bytes(),
    )?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use kvm_ioctls::VcpuExit;
    use vm_memory::Bytes;

    use super::*;
    use crate::arch::aarch64::vcpu::KvmVcpu;
    use crate::vstate::vm::tests::setup_vm_with_memory;

    #[test]
    fn test_guest_brk_delivery() {
        fn exception_state(pstate: u64, debug: bool) -> Vec<u64> {
            let mut vm = setup_vm_with_memory(0x3000);
            let mut vcpu = KvmVcpu::new(0, &vm).unwrap();
            vcpu.init(&[]).unwrap();
            vm.setup_irqchip(1).unwrap();
            let offset = match pstate & 0xf {
                0 => 0x400,
                4 => 0,
                5 => 0x200,
                _ => unreachable!(),
            };
            // BRK #0xf002; the EL1 handler writes to an unmapped MMIO address.
            vm.guest_memory()
                .write_obj(0xd43e0040_u32, GuestAddress(0))
                .unwrap();
            vm.guest_memory()
                .write_obj(0xd2a80000_u32, GuestAddress(0x1000 + offset))
                .unwrap();
            vm.guest_memory()
                .write_obj(0xf900001f_u32, GuestAddress(0x1004 + offset))
                .unwrap();
            if debug {
                vcpu_set_debug(&vcpu.fd, &[], false).unwrap();
            }
            for (reg, value) in [
                (PC_REG_ID, 0_u64),
                (PSTATE_ID, pstate),
                (0x603000000013c600, 0x1000),
            ] {
                vcpu.fd.set_one_reg(reg, &value.to_le_bytes()).unwrap();
            }
            if debug {
                let VcpuExit::Debug(exit) = vcpu.fd.run().unwrap() else {
                    panic!("Expected a trapped BRK")
                };
                vcpu_inject_bp(&vcpu.fd, &[], false, exit).unwrap();
            }
            let exit = vcpu.fd.run().unwrap();
            assert!(
                matches!(exit, VcpuExit::MmioWrite(0x40000000, _)),
                "{exit:?}"
            );
            [
                PC_REG_ID,
                PSTATE_ID,
                arm64_core_reg_id!(KVM_REG_SIZE_U64, offset_of!(kvm_regs, elr_el1)),
                arm64_core_reg_id!(KVM_REG_SIZE_U64, offset_of!(kvm_regs, spsr)),
                0x603000000013c290,
            ]
            .into_iter()
            .map(|reg| get_sys_reg(reg, &vcpu.fd).unwrap())
            .collect()
        }
        for mode in [0, 4, 5] {
            let pstate = 0xa00003c0 | mode;
            assert_eq!(
                exception_state(pstate, false),
                exception_state(pstate, true)
            );
        }
    }

    #[test]
    fn test_debug_controls_preserve_interrupt_masks() {
        let mut vm = setup_vm_with_memory(0x1000);
        let mut vcpu = KvmVcpu::new(0, &vm).unwrap();
        vcpu.init(&[]).unwrap();
        vm.setup_irqchip(1).unwrap();
        vm.guest_memory()
            .write_obj(0xd503201f_u32, GuestAddress(0))
            .unwrap();
        for masks in [0, 0x40, 0x80, 0xc0] {
            let pstate = 0xa0000305_u64 | masks;
            vcpu.fd
                .set_one_reg(PSTATE_ID, &pstate.to_le_bytes())
                .unwrap();
            for step in [false, true, false] {
                vcpu_set_debug(&vcpu.fd, &[], step).unwrap();
                assert_eq!(get_sys_reg(PSTATE_ID, &vcpu.fd).unwrap(), pstate);
                if step {
                    vcpu.fd
                        .set_one_reg(PC_REG_ID, &0_u64.to_le_bytes())
                        .unwrap();
                    assert!(matches!(vcpu.fd.run().unwrap(), VcpuExit::Debug(_)));
                    assert_eq!(get_instruction_pointer(&vcpu.fd).unwrap(), 4);
                    assert_eq!(get_sys_reg(PSTATE_ID, &vcpu.fd).unwrap(), pstate);
                }
            }
        }
    }
}
