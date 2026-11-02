// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: Apache-2.0

use super::super::protocol::Queue;
use super::super::test_utils::request;
use super::*;

fn memory() -> GuestMemoryMmap {
    crate::test_utils::single_region_mem(0x20000)
}

fn submit(
    renderer: &mut Renderer,
    mem: &GuestMemoryMmap,
    kind: u32,
    words: &[u32],
) -> Result<Reply, RendererError> {
    let bytes = request(kind, words);
    let queue = if kind >= 0x0300 {
        Queue::Cursor
    } else {
        Queue::Control
    };
    let (_, command) = Command::decode(queue, &bytes).unwrap();
    renderer.execute(command, mem)
}

#[test]
fn scatter_backing_transfer_and_partial_flush_preserve_other_pixels() {
    let mem = memory();
    let pixels = [
        1, 2, 3, 0, 4, 5, 6, 0, 7, 8, 9, 0, 10, 11, 12, 0, 13, 14, 15, 0, 16, 17, 18, 0,
    ];
    mem.write_slice(&pixels[..3], GuestAddress(0x1000)).unwrap();
    mem.write_slice(&pixels[3..], GuestAddress(0x2000)).unwrap();
    let mut renderer = Renderer::new(1024, 768).unwrap();
    submit(&mut renderer, &mem, 0x0101, &[1, 2, 3, 2]).unwrap();
    submit(
        &mut renderer,
        &mem,
        0x0106,
        &[1, 2, 0x1000, 0, 3, 0, 0x2000, 0, 21, 0],
    )
    .unwrap();
    submit(&mut renderer, &mem, 0x0103, &[1, 0, 2, 2, 0, 1]).unwrap();
    submit(&mut renderer, &mem, 0x0105, &[0, 0, 3, 2, 0, 0, 1, 0]).unwrap();
    assert_eq!(renderer.frame().unwrap().pixels, [0; 4]);
    submit(&mut renderer, &mem, 0x0104, &[1, 1, 1, 1, 1, 0]).unwrap();
    assert_eq!(renderer.frame().unwrap().pixels, [0, 0, 0x0f0e0d, 0]);
    submit(&mut renderer, &mem, 0x0104, &[0, 0, 3, 2, 1, 0]).unwrap();
    let frame = renderer.frame().unwrap();
    assert_eq!((frame.width, frame.height), (2, 2));
    assert_eq!(frame.pixels, [0x060504, 0x090807, 0x0f0e0d, 0x121110]);
}

#[test]
fn transfer_offset_is_the_first_source_pixel_and_rows_use_resource_stride() {
    let mem = memory();
    let pixels: Vec<_> = (0..24u8).collect();
    mem.write_slice(&pixels, GuestAddress(0x1000)).unwrap();
    let mut renderer = Renderer::new(1024, 768).unwrap();
    submit(&mut renderer, &mem, 0x0101, &[1, 2, 3, 2]).unwrap();
    submit(&mut renderer, &mem, 0x0106, &[1, 1, 0x1000, 0, 24, 0]).unwrap();
    submit(&mut renderer, &mem, 0x0105, &[1, 0, 1, 2, 4, 0, 1, 0]).unwrap();
    submit(&mut renderer, &mem, 0x0103, &[0, 0, 3, 2, 0, 1]).unwrap();
    assert_eq!(
        renderer.frame().unwrap().pixels,
        [0, 0x060504, 0, 0, 0x121110, 0]
    );
    assert_eq!(
        submit(
            &mut renderer,
            &mem,
            0x0105,
            &[1, 0, 1, 2, u32::MAX, u32::MAX, 1, 0]
        ),
        Err(RendererError::Parameter)
    );
}

#[test]
fn formats_produce_the_same_rgb_scanout() {
    let mem = memory();
    for (format, pixel) in [
        (1, [0x33, 0x22, 0x11, 0x80]),
        (2, [0x33, 0x22, 0x11, 0]),
        (3, [0x80, 0x11, 0x22, 0x33]),
        (4, [0, 0x11, 0x22, 0x33]),
        (67, [0x11, 0x22, 0x33, 0x80]),
        (68, [0, 0x33, 0x22, 0x11]),
        (121, [0x80, 0x33, 0x22, 0x11]),
        (134, [0x11, 0x22, 0x33, 0]),
    ] {
        mem.write_slice(&pixel, GuestAddress(0x1000)).unwrap();
        let mut renderer = Renderer::new(1024, 768).unwrap();
        submit(&mut renderer, &mem, 0x0101, &[1, format, 1, 1]).unwrap();
        submit(&mut renderer, &mem, 0x0106, &[1, 1, 0x1000, 0, 4, 0]).unwrap();
        submit(&mut renderer, &mem, 0x0105, &[0, 0, 1, 1, 0, 0, 1, 0]).unwrap();
        submit(&mut renderer, &mem, 0x0103, &[0, 0, 1, 1, 0, 1]).unwrap();
        assert_eq!(
            renderer.frame().unwrap().pixels,
            [0x112233],
            "format {format}"
        );
    }
}

#[test]
fn invalid_backing_and_rectangles_leave_the_existing_scanout_intact() {
    let mem = memory();
    let mut renderer = Renderer::new(1024, 768).unwrap();
    submit(&mut renderer, &mem, 0x0101, &[1, 2, 2, 2]).unwrap();
    submit(&mut renderer, &mem, 0x0103, &[0, 0, 2, 2, 0, 1]).unwrap();
    for words in [
        [1, 1, 0x1ffff, 0, 16, 0],
        [1, 1, u32::MAX, u32::MAX, 16, 0],
        [1, 1, 0x1000, 0, 0, 0],
        [1, 1, 0x1000, 0, 4, 0],
    ] {
        assert_eq!(
            submit(&mut renderer, &mem, 0x0106, &words),
            Err(RendererError::Parameter)
        );
    }
    submit(&mut renderer, &mem, 0x0106, &[1, 1, 0x1000, 0, 16, 0]).unwrap();
    assert_eq!(
        submit(&mut renderer, &mem, 0x0106, &[1, 1, 0x2000, 0, 16, 0]),
        Err(RendererError::Parameter)
    );
    for rect in [
        [1, 1, 2, 2],
        [u32::MAX, 0, 1, 1],
        [0, u32::MAX, 1, 1],
        [0, 0, u32::MAX, u32::MAX],
    ] {
        let [x, y, w, h] = rect;
        assert_eq!(
            submit(&mut renderer, &mem, 0x0103, &[x, y, w, h, 0, 1]),
            Err(RendererError::Parameter)
        );
        assert_eq!(
            submit(&mut renderer, &mem, 0x0105, &[x, y, w, h, 0, 0, 1, 0]),
            Err(RendererError::Parameter)
        );
    }
    assert_eq!(renderer.frame().unwrap().pixels, [0; 4]);
    submit(&mut renderer, &mem, 0x0107, &[1, 0]).unwrap();
    assert_eq!(
        submit(&mut renderer, &mem, 0x0105, &[0, 0, 2, 2, 0, 0, 1, 0]),
        Err(RendererError::Parameter)
    );
    submit(&mut renderer, &mem, 0x0106, &[1, 1, 0x2000, 0, 16, 0]).unwrap();
}

#[test]
fn resource_and_byte_limits_are_released_by_unref_and_reset() {
    let mem = memory();
    let mut renderer = Renderer::new(1024, 768).unwrap();
    for dimensions in [[0, 1], [1, 0], [u32::MAX, u32::MAX]] {
        assert_eq!(
            submit(
                &mut renderer,
                &mem,
                0x0101,
                &[1, 2, dimensions[0], dimensions[1]]
            ),
            Err(RendererError::Parameter)
        );
    }
    assert_eq!(
        submit(&mut renderer, &mem, 0x0101, &[1, 2, 4096, 4096]),
        Err(RendererError::OutOfMemory)
    );
    for id in 1..=2 {
        submit(&mut renderer, &mem, 0x0101, &[id, 2, 4096, 2048]).unwrap();
    }
    assert_eq!(
        submit(&mut renderer, &mem, 0x0101, &[3, 2, 1, 1]),
        Err(RendererError::OutOfMemory)
    );
    assert_eq!(
        submit(&mut renderer, &mem, 0x0101, &[1, 2, 1, 1]),
        Err(RendererError::Resource)
    );
    submit(&mut renderer, &mem, 0x0102, &[1, 0]).unwrap();
    submit(&mut renderer, &mem, 0x0101, &[3, 2, 1, 1]).unwrap();
    renderer.reset();
    assert_eq!(renderer.total_bytes, 0);
    for id in 1..=u32::try_from(MAX_RESOURCES).unwrap() {
        submit(&mut renderer, &mem, 0x0101, &[id, 2, 1, 1]).unwrap();
    }
    assert_eq!(
        submit(&mut renderer, &mem, 0x0101, &[100, 2, 1, 1]),
        Err(RendererError::OutOfMemory)
    );
}

#[test]
fn cursor_is_copied_clipped_blended_moved_and_hidden() {
    let mem = memory();
    let mut renderer = Renderer::new(1024, 768).unwrap();
    mem.write_slice(&[0x30, 0x20, 0x10, 0], GuestAddress(0x1000))
        .unwrap();
    submit(&mut renderer, &mem, 0x0101, &[1, 2, 1, 1]).unwrap();
    submit(&mut renderer, &mem, 0x0106, &[1, 1, 0x1000, 0, 4, 0]).unwrap();
    submit(&mut renderer, &mem, 0x0105, &[0, 0, 1, 1, 0, 0, 1, 0]).unwrap();
    submit(&mut renderer, &mem, 0x0103, &[0, 0, 1, 1, 0, 1]).unwrap();
    submit(&mut renderer, &mem, 0x0101, &[2, 1, 64, 64]).unwrap();
    submit(&mut renderer, &mem, 0x0106, &[2, 1, 0x10000, 0, 16384, 0]).unwrap();
    mem.write_slice(&[16, 32, 64, 128], GuestAddress(0x10000 + 63 * 4))
        .unwrap();
    submit(&mut renderer, &mem, 0x0105, &[0, 0, 64, 64, 0, 0, 2, 0]).unwrap();
    submit(&mut renderer, &mem, 0x0300, &[0, 0, 0, 0, 2, 63, 0, 0]).unwrap();
    assert_eq!(renderer.frame().unwrap().pixels, [0x483028]);
    submit(&mut renderer, &mem, 0x0102, &[2, 0]).unwrap();
    assert_eq!(renderer.frame().unwrap().pixels, [0x483028]);
    submit(
        &mut renderer,
        &mem,
        0x0301,
        &[0, u32::MAX, u32::MAX, 0, 0, 0, 0, 0],
    )
    .unwrap();
    assert_eq!(renderer.frame().unwrap().pixels, [0x102030]);
    submit(&mut renderer, &mem, 0x0300, &[0; 8]).unwrap();
    assert!(renderer.cursor.is_none());
}

#[test]
fn mode_change_reset_and_scanout_disable_have_separate_lifetimes() {
    let mem = memory();
    let mut renderer = Renderer::new(1024, 768).unwrap();
    assert!(!renderer.is_native());
    assert!(!renderer.resize(1024, 768).unwrap());
    assert!(renderer.resize(1920, 1080).unwrap());
    assert_eq!(
        submit(&mut renderer, &mem, 0x0100, &[]),
        Ok(Reply::DisplayInfo {
            width: 1920,
            height: 1080
        })
    );
    submit(&mut renderer, &mem, 0x0103, &[0; 6]).unwrap();
    assert!(renderer.is_native());
    assert!(renderer.frame().is_none());
    assert_eq!(
        submit(&mut renderer, &mem, 0x0103, &[0, 0, 0, 0, 1, 0]),
        Err(RendererError::Scanout)
    );
    renderer.reset();
    assert!(!renderer.is_native());
    assert_eq!(renderer.preferred, (1920, 1080));
}

fn saved_pixel(mem: &GuestMemoryMmap) -> RendererState {
    let mut renderer = Renderer::new(1024, 768).unwrap();
    mem.write_slice(&[0, 0, 255, 0], GuestAddress(0x1000))
        .unwrap();
    submit(&mut renderer, mem, 0x0101, &[1, 2, 1, 1]).unwrap();
    submit(&mut renderer, mem, 0x0106, &[1, 1, 0x1000, 0, 4, 0]).unwrap();
    submit(&mut renderer, mem, 0x0105, &[0, 0, 1, 1, 0, 0, 1, 0]).unwrap();
    submit(&mut renderer, mem, 0x0103, &[0, 0, 1, 1, 0, 1]).unwrap();
    renderer.cursor = Some(Cursor {
        x: u32::MAX,
        y: u32::MAX,
        hot_x: 63,
        hot_y: 63,
        pixels: vec![0x80008000; (CURSOR_SIZE * CURSOR_SIZE) as usize],
    });
    renderer.save()
}

#[test]
fn restore_keeps_unflushed_resource_pixels_scanout_cursor_and_guest_backing() {
    let mem = memory();
    let mut renderer = Renderer::restore(&saved_pixel(&mem), &mem).unwrap();
    mem.write_slice(&[255, 0, 0, 0], GuestAddress(0x1000))
        .unwrap();
    submit(&mut renderer, &mem, 0x0105, &[0, 0, 1, 1, 0, 0, 1, 0]).unwrap();
    let bytes = bitcode::serialize(&renderer.save()).unwrap();
    let state: RendererState = bitcode::deserialize(&bytes).unwrap();
    mem.write_slice(&[0, 255, 0, 0], GuestAddress(0x1000))
        .unwrap();
    let mut restored = Renderer::restore(&state, &mem).unwrap();
    assert!(restored.is_native());
    assert_eq!(restored.total_bytes, 4);
    assert_eq!(restored.preferred, (1024, 768));
    let cursor = restored.cursor.as_ref().unwrap();
    assert_eq!(
        (cursor.x, cursor.y, cursor.hot_x, cursor.hot_y),
        (u32::MAX, u32::MAX, 63, 63)
    );
    assert_eq!(cursor.pixels, vec![0x80008000; 4096]);
    assert_eq!(restored.frame().unwrap().pixels, [0xff0000]);
    submit(&mut restored, &mem, 0x0104, &[0, 0, 1, 1, 1, 0]).unwrap();
    assert_eq!(restored.frame().unwrap().pixels, [0x0000ff]);
    submit(&mut restored, &mem, 0x0105, &[0, 0, 1, 1, 0, 0, 1, 0]).unwrap();
    submit(&mut restored, &mem, 0x0104, &[0, 0, 1, 1, 1, 0]).unwrap();
    assert_eq!(restored.frame().unwrap().pixels, [0x00ff00]);
}

#[test]
fn restore_rejects_inconsistent_resources_backing_scanout_and_cursor() {
    let mem = memory();
    let state = saved_pixel(&mem);
    let cases: &[fn(&mut RendererState)] = &[
        |state| state.preferred = (0, 768),
        |state| state.resources.push(state.resources[0].clone()),
        |state| {
            state
                .resources
                .resize(MAX_RESOURCES + 1, state.resources[0].clone())
        },
        |state| state.resources[0].width = u32::MAX,
        |state| state.resources[0].format = 0,
        |state| state.pixels.clear(),
        |state| state.backing[0].length = 0,
        |state| state.backing[0].address = u64::MAX,
        |state| state.backing[0].address = 0x20000,
        |state| {
            state.backing.resize(
                MAX_BACKING_ENTRIES + 1,
                BackingEntry {
                    address: 0x1000,
                    length: 4,
                },
            )
        },
        |state| state.scanout.as_mut().unwrap().resource = ResourceId::new(2).unwrap(),
        |state| state.scanout.as_mut().unwrap().rect.width = 0,
        |state| state.scanout.as_mut().unwrap().rect.x = u32::MAX,
        |state| state.scanout.as_mut().unwrap().pixels.clear(),
        |state| state.scanout.as_mut().unwrap().pixels[0] |= 0xff000000,
        |state| state.cursor.as_mut().unwrap().pixels.clear(),
        |state| state.cursor.as_mut().unwrap().hot_x = 64,
        |state| state.native = false,
    ];
    for (index, modify) in cases.iter().enumerate() {
        let mut invalid = state.clone();
        modify(&mut invalid);
        assert!(Renderer::restore(&invalid, &mem).is_err(), "case {index}");
    }
    Renderer::restore(&state, &mem).unwrap();
}

#[test]
fn restore_checks_the_aggregate_resource_budget_before_cloning_pixels() {
    let mem = memory();
    let mut state = Renderer::new(1024, 768).unwrap().save();
    state.pixels = vec![0; MAX_TOTAL_BYTES + 4];
    state.resources = vec![
        ResourceState {
            id: ResourceId::new(1).unwrap(),
            width: 4096,
            height: 2048,
            format: 2,
            pixels: 0..MAX_RESOURCE_BYTES,
            backing: 0..0,
        },
        ResourceState {
            id: ResourceId::new(2).unwrap(),
            width: 4096,
            height: 2048,
            format: 2,
            pixels: MAX_RESOURCE_BYTES..MAX_TOTAL_BYTES,
            backing: 0..0,
        },
        ResourceState {
            id: ResourceId::new(3).unwrap(),
            width: 1,
            height: 1,
            format: 2,
            pixels: MAX_TOTAL_BYTES..MAX_TOTAL_BYTES + 4,
            backing: 0..0,
        },
    ];
    assert_eq!(
        Renderer::restore(&state, &mem).unwrap_err(),
        RendererError::OutOfMemory
    );
}

#[test]
fn checkpoint_decoder_rejects_excess_pixels_before_resource_restore() {
    let mem = memory();
    let mut state = saved_pixel(&mem);
    state.pixels.resize(MAX_TOTAL_BYTES + 1, 0);
    let encoded = bitcode::serialize(&state).unwrap();
    bitcode::deserialize::<RendererState>(&encoded).unwrap_err();
}

#[test]
fn checkpoint_decoder_bounds_metadata_backing_scanout_and_cursor() {
    let mem = memory();
    let original = saved_pixel(&mem);
    let cases: &[fn(&mut RendererState)] = &[
        |state| {
            state
                .resources
                .resize(MAX_RESOURCES + 1, state.resources[0].clone())
        },
        |state| {
            state.backing.resize(
                MAX_RESOURCES * MAX_BACKING_ENTRIES + 1,
                BackingEntry {
                    address: 0x1000,
                    length: 4,
                },
            )
        },
        |state| {
            state
                .scanout
                .as_mut()
                .unwrap()
                .pixels
                .resize(MAX_RESOURCE_BYTES / 4 + 1, 0)
        },
        |state| state.cursor.as_mut().unwrap().pixels.resize(4097, 0),
    ];
    for modify in cases {
        let mut state = original.clone();
        modify(&mut state);
        let encoded = bitcode::serialize(&state).unwrap();
        bitcode::deserialize::<RendererState>(&encoded).unwrap_err();
    }
}

#[test]
fn restore_rejects_overlapping_gapped_reversed_and_unowned_ranges() {
    let mem = memory();
    let original = saved_pixel(&mem);
    let cases: &[fn(&mut RendererState)] = &[
        |state| state.resources[0].pixels = 1..4,
        |state| state.resources[0].pixels = std::ops::Range { start: 4, end: 0 },
        |state| state.resources[0].backing = 1..1,
        |state| state.resources[0].backing = 0..2,
        |state| state.pixels.push(0),
        |state| {
            state.backing.push(BackingEntry {
                address: 0x1000,
                length: 4,
            })
        },
        |state| {
            let mut duplicate = state.resources[0].clone();
            duplicate.id = ResourceId::new(2).unwrap();
            state.resources.push(duplicate);
        },
    ];
    for modify in cases {
        let mut state = original.clone();
        modify(&mut state);
        Renderer::restore(&state, &mem).unwrap_err();
    }
}
