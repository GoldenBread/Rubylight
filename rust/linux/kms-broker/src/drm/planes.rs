//! The cursor plane of a display controller: which plane it is (its `type` property) and where
//! it sits (`CRTC_X`, `CRTC_Y`, `CRTC_W`, `CRTC_H`). The pointer is never in the framebuffer a
//! compositor scans out, so a capture draws this plane over it.
use super::{Card, iowr};
use anyhow::{Context, Result};
use std::collections::HashMap;

/// `DRM_MODE_OBJECT_PLANE`.
const OBJECT_PLANE: u32 = 0xeeee_eeee;
/// The `type` property's value for a cursor plane (`DRM_PLANE_TYPE_CURSOR`).
const TYPE_CURSOR: u64 = 2;

#[repr(C)]
#[derive(Default)]
struct ObjGetProperties {
    props_ptr: u64,
    prop_values_ptr: u64,
    count_props: u32,
    obj_id: u32,
    obj_type: u32,
}

#[repr(C)]
struct GetProperty {
    values_ptr: u64,
    enum_blob_ptr: u64,
    prop_id: u32,
    flags: u32,
    name: [u8; 32],
    count_values: u32,
    count_enum_blobs: u32,
}

const OBJ_GET_PROPERTIES: u64 = iowr(0xb9, size_of::<ObjGetProperties>());
const GET_PROPERTY: u64 = iowr(0xaa, size_of::<GetProperty>());

/// The cursor plane as it is shown now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cursor {
    pub plane_id: u32,
    pub fb_id: u32,
    /// Its top-left corner on the output, which may be off the output's edge.
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

impl Card {
    /// A property's name, read once per property and kept: names never change.
    fn property_name(&self, id: u32) -> Option<String> {
        let mut names = self.names.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(name) = names.get(&id) {
            return Some(name.clone());
        }
        let mut property = GetProperty {
            values_ptr: 0,
            enum_blob_ptr: 0,
            prop_id: id,
            flags: 0,
            name: [0; 32],
            count_values: 0,
            count_enum_blobs: 0,
        };
        self.ioctl(GET_PROPERTY, &mut property).ok()?;
        let end = property.name.iter().position(|&b| b == 0).unwrap_or(32);
        let name = String::from_utf8_lossy(&property.name[..end]).into_owned();
        names.insert(id, name.clone());
        Some(name)
    }

    /// A plane's properties by name.
    fn plane_properties(&self, plane_id: u32) -> Result<HashMap<String, u64>> {
        let mut count = ObjGetProperties {
            obj_id: plane_id,
            obj_type: OBJECT_PLANE,
            ..ObjGetProperties::default()
        };
        self.ioctl(OBJ_GET_PROPERTIES, &mut count)
            .with_context(|| format!("counting plane {plane_id}'s properties"))?;
        let mut ids = vec![0u32; count.count_props as usize];
        let mut values = vec![0u64; ids.len()];
        let mut read = ObjGetProperties {
            props_ptr: ids.as_mut_ptr() as u64,
            prop_values_ptr: values.as_mut_ptr() as u64,
            count_props: ids.len() as u32,
            obj_id: plane_id,
            obj_type: OBJECT_PLANE,
        };
        self.ioctl(OBJ_GET_PROPERTIES, &mut read)
            .with_context(|| format!("reading plane {plane_id}'s properties"))?;
        let shown = (read.count_props as usize).min(ids.len());
        Ok(ids[..shown]
            .iter()
            .zip(&values[..shown])
            .filter_map(|(&id, &value)| Some((self.property_name(id)?, value)))
            .collect())
    }

    /// The cursor plane of `crtc_id` when it shows something.
    pub fn cursor(&self, crtc_id: u32) -> Result<Option<Cursor>> {
        for plane in self.planes(crtc_id)? {
            if plane.fb_id == 0 {
                continue;
            }
            let properties = self.plane_properties(plane.plane_id)?;
            if properties.get("type") != Some(&TYPE_CURSOR) {
                continue;
            }
            let value = |name: &str| properties.get(name).copied().unwrap_or_default();
            return Ok(Some(Cursor {
                plane_id: plane.plane_id,
                fb_id: plane.fb_id,
                // Signed range properties hold their value sign-extended.
                x: value("CRTC_X") as i64 as i32,
                y: value("CRTC_Y") as i64 as i32,
                width: value("CRTC_W") as u32,
                height: value("CRTC_H") as u32,
            }));
        }
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn structures_match_the_kernels() {
        assert_eq!(size_of::<ObjGetProperties>(), 32);
        assert_eq!(size_of::<GetProperty>(), 64);
        assert_eq!(OBJ_GET_PROPERTIES, 0xc020_64b9);
        assert_eq!(GET_PROPERTY, 0xc040_64aa);
    }
}
