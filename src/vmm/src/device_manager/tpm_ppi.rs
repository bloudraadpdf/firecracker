// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: Apache-2.0

use acpi_tables::{Aml, aml};

use crate::devices::uefi_vars::MMIO_BASE;

type Result<T> = std::result::Result<T, aml::AmlError>;

struct Terms(Vec<u8>);

impl Terms {
    fn new() -> Self {
        Self(Vec::new())
    }

    fn push(&mut self, term: &dyn Aml) -> Result<()> {
        term.append_aml_bytes(&mut self.0)
    }
}

impl Aml for Terms {
    fn append_aml_bytes(&self, bytes: &mut Vec<u8>) -> Result<()> {
        bytes.extend_from_slice(&self.0);
        Ok(())
    }
}

fn path(name: &str) -> Result<aml::Path> {
    name.try_into()
}

fn branch(condition: &dyn Aml, body: &dyn Aml) -> Result<Terms> {
    let mut terms = Terms::new();
    terms.push(&aml::If::new(condition, vec![body]))?;
    Ok(terms)
}

fn equal_return(value: u8, result: &dyn Aml) -> Result<Terms> {
    branch(
        &aml::Equal::new(&aml::Arg(2), &value),
        &aml::Return::new(result),
    )
}

fn package(values: &[&dyn Aml]) -> Result<Terms> {
    let mut terms = Terms::new();
    terms.push(&aml::Store::new(
        &aml::Local(0),
        &aml::Package::new(vec![&0u8; values.len()]),
    ))?;
    for (index, value) in values.iter().enumerate() {
        terms.push(&aml::Store::new(
            &aml::Index::new(&aml::Zero {}, &aml::Local(0), &index),
            *value,
        ))?;
    }
    terms.push(&aml::Return::new(&aml::Local(0)))?;
    Ok(terms)
}

fn response(code: &dyn Aml, first: &dyn Aml, second: &dyn Aml) -> Result<Terms> {
    let mut terms = Terms::new();
    let three = package(&[code, first, second])?;
    let two = package(&[code, first])?;
    let mut pending = Terms::new();
    pending.push(&branch(&aml::Equal::new(&aml::Arg(1), &2u8), &three)?)?;
    pending.push(&two)?;
    terms.push(&branch(&aml::Equal::new(&aml::Arg(2), &3u8), &pending)?)?;
    terms.push(&branch(&aml::Equal::new(&aml::Arg(2), &5u8), &three)?)?;
    terms.push(&aml::Return::new(code))?;
    Ok(terms)
}

/// PPI 1.30 _DSM; all helper waits yield the interpreter and have a finite limit.
pub(super) fn append(bytes: &mut Vec<u8>) -> Result<()> {
    let mut children = Terms::new();
    children.push(&aml::OpRegion::new(
        path("PPIR")?,
        aml::OpRegionSpace::SystemMemory,
        usize::try_from(MMIO_BASE + 0x100).unwrap(),
        32,
    ))?;
    children.push(&aml::Field::new(
        path("PPIR")?,
        aml::FieldAccessType::DWord,
        aml::FieldUpdateRule::Preserve,
        [
            *b"PSTS", *b"PFUN", *b"PREV", *b"POPR", *b"PARG", *b"PRET", *b"PVAL", *b"PRES",
        ]
        .into_iter()
        .map(|name| aml::FieldEntry::Named(name, 32))
        .collect(),
    ))?;
    let mut body = Terms::new();
    let unsupported_buffer = aml::Buffer::new(vec![0]);
    let unsupported = aml::Return::new(&unsupported_buffer);
    // Guard operand types before UUID comparison or package indexing.
    for (argument, kind) in [(0, 3u8), (1, 1), (2, 1)] {
        body.push(&branch(
            &aml::Not(&aml::Equal::new(
                &aml::ObjectType(&aml::Arg(argument)),
                &kind,
            )),
            &unsupported,
        )?)?;
    }
    let uuid = aml::Buffer::new(vec![
        0xa6, 0xfa, 0xdd, 0x3d, 0x1b, 0x36, 0xb4, 0x4e, 0xa4, 0x24, 0x8d, 0x10, 0x08, 0x9d, 0x16,
        0x53,
    ]);
    body.push(&branch(
        &aml::Not(&aml::Equal::new(&aml::Arg(0), &uuid)),
        &unsupported,
    )?)?;
    let mut revision_two = Terms::new();
    revision_two.push(&equal_return(0, &aml::Buffer::new(vec![0x89, 0]))?)?;
    revision_two.push(&branch(
        &aml::Not(&aml::Equal::new(&aml::Arg(2), &3u8)),
        &branch(
            &aml::Not(&aml::Equal::new(&aml::Arg(2), &7u8)),
            &unsupported,
        )?,
    )?)?;
    body.push(&branch(
        &aml::Equal::new(&aml::Arg(1), &2u8),
        &revision_two,
    )?)?;
    body.push(&branch(
        &aml::Not(&aml::Equal::new(&aml::Arg(1), &1u8)),
        &branch(
            &aml::Not(&aml::Equal::new(&aml::Arg(1), &2u8)),
            &unsupported,
        )?,
    )?)?;
    body.push(&equal_return(0, &aml::Buffer::new(vec![0xff, 1]))?)?;
    body.push(&equal_return(1, &"1.3")?)?;
    body.push(&equal_return(4, &2u8)?)?;
    body.push(&equal_return(6, &3u8)?)?;
    body.push(&branch(
        &aml::LessThan::new(&8u8, &aml::Arg(2)),
        &unsupported,
    )?)?;

    let mut failure = Terms::new();
    failure.push(&equal_return(8, &0u8)?)?;
    failure.push(&equal_return(2, &2u8)?)?;
    failure.push(&equal_return(7, &2u8)?)?;
    failure.push(&response(&1u8, &0u8, &0u8)?)?;
    body.push(&aml::Store::new(&aml::Local(1), &0u8))?;
    body.push(&aml::Store::new(&aml::Local(2), &0u8))?;
    let mut operation = Terms::new();
    operation.push(&branch(
        &aml::Not(&aml::Equal::new(&aml::ObjectType(&aml::Arg(3)), &4u8)),
        &failure,
    )?)?;
    operation.push(&branch(
        &aml::LessThan::new(&aml::SizeOf(&aml::Arg(3)), &1u8),
        &failure,
    )?)?;
    operation.push(&aml::Store::new(
        &aml::Local(1),
        &aml::DerefOf(&aml::Index::new(&aml::Zero {}, &aml::Arg(3), &0u8)),
    ))?;
    operation.push(&branch(
        &aml::Not(&aml::Equal::new(&aml::ObjectType(&aml::Local(1)), &1u8)),
        &failure,
    )?)?;
    operation.push(&branch(
        &aml::LessThan::new(&u32::MAX, &aml::Local(1)),
        &failure,
    )?)?;
    for function in [2u8, 7, 8] {
        body.push(&branch(
            &aml::Equal::new(&aml::Arg(2), &function),
            &operation,
        )?)?;
    }
    let mut parameter = Terms::new();
    parameter.push(&branch(
        &aml::LessThan::new(&aml::SizeOf(&aml::Arg(3)), &2u8),
        &failure,
    )?)?;
    parameter.push(&aml::Store::new(
        &aml::Local(2),
        &aml::DerefOf(&aml::Index::new(&aml::Zero {}, &aml::Arg(3), &1u8)),
    ))?;
    parameter.push(&branch(
        &aml::Not(&aml::Equal::new(&aml::ObjectType(&aml::Local(2)), &1u8)),
        &failure,
    )?)?;
    parameter.push(&branch(
        &aml::LessThan::new(&u32::MAX, &aml::Local(2)),
        &failure,
    )?)?;
    body.push(&branch(
        &aml::Equal::new(&aml::Arg(2), &7u8),
        &branch(&aml::Equal::new(&aml::Arg(1), &2u8), &parameter)?,
    )?)?;
    // A prior timed-out call can still own the mailbox. Do not reuse its reply.
    body.push(&branch(&aml::Equal::new(&path("PSTS")?, &1u8), &failure)?)?;
    for (name, value) in [
        ("PFUN", &aml::Arg(2) as &dyn Aml),
        ("PREV", &aml::Arg(1)),
        ("POPR", &aml::Local(1)),
        ("PARG", &aml::Local(2)),
    ] {
        body.push(&aml::Store::new(&path(name)?, value))?;
    }
    body.push(&aml::Store::new(&path("PSTS")?, &1u8))?;
    body.push(&aml::Store::new(&aml::Local(3), &0u8))?;
    let mut poll = Terms::new();
    poll.push(&branch(
        &aml::Equal::new(&aml::Local(3), &10_000u32),
        &failure,
    )?)?;
    poll.push(&aml::Sleep(&1u8))?;
    poll.push(&aml::Add::new(&aml::Local(3), &aml::Local(3), &1u8))?;
    body.push(&aml::While::new(
        &aml::Equal::new(&path("PSTS")?, &1u8),
        vec![&poll],
    ))?;
    body.push(&branch(
        &aml::Not(&aml::Equal::new(&path("PSTS")?, &0u8)),
        &failure,
    )?)?;
    body.push(&response(&path("PRET")?, &path("PVAL")?, &path("PRES")?)?)?;
    children.push(&aml::Method::new(path("_DSM")?, 4, true, vec![&body]))?;
    aml::Scope::new(path("_SB_.TPM0")?, vec![&children]).append_aml_bytes(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn contains(bytes: &[u8], expected: &[u8]) -> bool {
        bytes
            .windows(expected.len())
            .any(|window| window == expected)
    }

    #[test]
    fn dsm_is_serialized_and_mailbox_and_poll_are_bounded() {
        let mut bytes = Vec::new();
        append(&mut bytes).unwrap();
        assert!(contains(&bytes, b"_DSM\x0c"));
        let mut region = b"PPIR\x00\x0c".to_vec();
        region.extend_from_slice(&u32::try_from(MMIO_BASE + 0x100).unwrap().to_le_bytes());
        region.extend_from_slice(&[0x0a, 0x20]);
        assert!(contains(&bytes, &region));
        assert!(contains(
            &bytes,
            b"PPIR\x03PSTS\x20PFUN\x20PREV\x20POPR\x20PARG\x20PRET\x20PVAL\x20PRES\x20"
        ));
        assert!(contains(
            &bytes,
            &[
                0xa6, 0xfa, 0xdd, 0x3d, 0x1b, 0x36, 0xb4, 0x4e, 0xa4, 0x24, 0x8d, 0x10, 0x08, 0x9d,
                0x16, 0x53
            ]
        ));
        assert!(contains(&bytes, &[0x5b, 0x22, 0x0a, 0x01]));
        assert!(contains(&bytes, &[0x93, 0x63, 0x0c, 0x10, 0x27, 0, 0]));
    }
    #[test]
    fn acpica_executes_dsm_and_response_packages() {
        use acpi_tables::Sdt;
        use std::io::Write;
        use std::process::Command;
        use vm_memory::{Bytes, GuestAddress};

        let mut bytes = Vec::new();
        aml::Device::new(
            path("_SB_.TPM0").unwrap(),
            vec![&aml::Name::new(path("_HID").unwrap(), &"MSFT0101").unwrap()],
        )
        .append_aml_bytes(&mut bytes)
        .unwrap();
        append(&mut bytes).unwrap();
        let returned = response(&0u8, &21u8, &42u8).unwrap();
        aml::Method::new(path("RSPN").unwrap(), 3, true, vec![&returned])
            .append_aml_bytes(&mut bytes)
            .unwrap();
        let mut table = acpi_tables::Dsdt::new(*b"FC    ", *b"PPI TEST", 1, bytes);
        let memory =
            vm_memory::GuestMemoryMmap::<()>::from_ranges(&[(GuestAddress(0), 0x10000)]).unwrap();
        table.write_to_guest(&memory, GuestAddress(0)).unwrap();
        let mut bytes = vec![0; table.len()];
        memory.read_slice(&mut bytes, GuestAddress(0)).unwrap();
        let file = vmm_sys_util::tempfile::TempFile::new_in(std::path::Path::new(".")).unwrap();
        file.as_file().write_all(&bytes).unwrap();
        let uuid = "(A6 FA DD 3D 1B 36 B4 4E A4 24 8D 10 08 9D 16 53)";
        for (arguments, expected) in [
            (format!("{uuid} 1 0 []"), "FF 01"),
            (format!("{uuid} 2 0 []"), "89 00"),
            (format!("{uuid} 1 1 []"), "1.3"),
            (format!("{uuid} 1 4 (00)"), "0000000000000002"),
            (format!("{uuid} 1 6 []"), "0000000000000003"),
            (format!("{uuid} 1 3 (00)"), "Contains 2 Elements"),
            (format!("{uuid} 1 2 []"), "0000000000000002"),
            (format!("{uuid} 2 7 [5]"), "0000000000000002"),
            (format!("{uuid} 1 8 [\"bad\"]"), "0000000000000000"),
            (format!("{uuid} 3 0 []"), "00"),
            ("(00) 1 0 []".into(), "00"),
        ] {
            let command = format!("execute \\_SB.TPM0._DSM {arguments}");
            let output = Command::new("acpiexec")
                .args(["-to", "20", "-b", &command])
                .arg(file.as_path())
                .output()
                .unwrap();
            let output = format!(
                "{}{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            assert!(!output.contains("AE_"), "{command}: {output}");
            assert!(output.contains(expected), "{command}: {output}");
        }
        for (revision, function, count) in [(1, 3, 2), (2, 3, 3), (1, 5, 3)] {
            let command = format!("execute RSPN 0 {revision} {function}");
            let output = Command::new("acpiexec")
                .args(["-to", "20", "-b", &command])
                .arg(file.as_path())
                .output()
                .unwrap();
            let output = String::from_utf8_lossy(&output.stdout);
            assert!(!output.contains("AE_"), "{output}");
            assert!(
                output.contains(&format!("Contains {count} Elements")),
                "{output}"
            );
            assert!(output.contains("0000000000000015"), "{output}");
            if count == 3 {
                assert!(output.contains("000000000000002A"), "{output}");
            }
        }
    }
}
