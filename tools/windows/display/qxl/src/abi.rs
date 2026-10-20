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
        output.copy_from_slice(&(self as usize).to_le_bytes());
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
