// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: GPL-2.0-only

//! Native record encoding without copying Rust padding to Windows.

pub(crate) trait Field: Copy {
    fn zero() -> Self;
    fn write(self, output: &mut [u8]);
}

macro_rules! integer {
    ($($ty:ty),+) => {$(
        impl Field for $ty {
            fn zero() -> Self { 0 }
            fn write(self, output: &mut [u8]) {
                output.copy_from_slice(&self.to_le_bytes());
            }
        }
    )+};
}
integer!(u8, u16, u32, i32, u64);

impl<T> Field for *mut T {
    fn zero() -> Self {
        core::ptr::null_mut()
    }
    fn write(self, output: &mut [u8]) {
        let output = &mut output[..size_of::<Self>()];
        // SAFETY: thin native pointers have no padding. Copy their initialized
        // representation without converting to integers and losing provenance;
        // the bounded destination is distinct from this by-value local.
        unsafe {
            core::ptr::copy_nonoverlapping(
                core::ptr::addr_of!(self).cast::<u8>(),
                output.as_mut_ptr(),
                output.len(),
            )
        };
    }
}

impl<T: Field, const N: usize> Field for [T; N] {
    fn zero() -> Self {
        [T::zero(); N]
    }
    fn write(self, output: &mut [u8]) {
        for (index, field) in self.into_iter().enumerate() {
            let start = index * size_of::<T>();
            field.write(&mut output[start..start + size_of::<T>()]);
        }
    }
}

macro_rules! record {
    ($name:ident { $($field:ident: $ty:ty),+ $(,)? }) => {
        #[repr(C)]
        #[derive(Clone, Copy, Debug)]
        pub struct $name { $(pub $field: $ty),+ }

        impl Default for $name {
            fn default() -> Self {
                Self { $($field: <$ty as $crate::abi::Field>::zero()),+ }
            }
        }

        impl $crate::abi::Field for $name {
            fn zero() -> Self { Self::default() }
            fn write(self, output: &mut [u8]) {
                output.fill(0);
                $(
                    let offset = core::mem::offset_of!(Self, $field);
                    $crate::abi::Field::write(self.$field,
                        &mut output[offset..offset + core::mem::size_of::<$ty>()]);
                )+
            }
        }

        impl $name {
            /// Encode fields and clear padding; short buffers remain unchanged.
            pub fn encode(&self, output: &mut [u8]) -> Option<usize> {
                let size = core::mem::size_of::<Self>();
                let output = output.get_mut(..size)?;
                $crate::abi::Field::write(*self, output);
                Some(size)
            }
        }
    };
}
pub(crate) use record;

#[cfg(test)]
mod tests {
    record! { PointerRecord { value: *mut u8 }}

    #[test]
    fn native_pointer_fields_retain_their_pointee() {
        let mut value = 0x5au8;
        let input = PointerRecord {
            value: core::ptr::from_mut(&mut value),
        };
        let mut output = PointerRecord::default();
        // SAFETY: this native record has only one initialized pointer field,
        // with no padding. The exclusive byte view lasts only through encode.
        let bytes = unsafe {
            core::slice::from_raw_parts_mut(
                core::ptr::from_mut(&mut output).cast::<u8>(),
                size_of::<PointerRecord>(),
            )
        };
        input.encode(bytes).unwrap();
        // SAFETY: encoding must preserve the original live pointee, not merely
        // its numerical address. No other reference is used during this write.
        unsafe { output.value.write(0x7b) };
        assert_eq!(value, 0x7b);
    }
}
