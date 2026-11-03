use crate::{CGRect, id};
use anyhow::Result;
use gpui::{Bounds, DisplayId, Pixels, PlatformDisplay, point, px, size};
use objc::{class, msg_send, sel, sel_impl};

/// The device's built-in screen.
#[derive(Debug)]
pub(crate) struct IosDisplay;

impl PlatformDisplay for IosDisplay {
    fn id(&self) -> DisplayId {
        DisplayId::new(1)
    }

    fn uuid(&self) -> Result<uuid::Uuid> {
        Ok(uuid::Uuid::nil())
    }

    fn bounds(&self) -> Bounds<Pixels> {
        let frame: CGRect = unsafe {
            let screen: id = msg_send![class!(UIScreen), mainScreen];
            msg_send![screen, bounds]
        };
        Bounds::new(
            point(px(0.), px(0.)),
            size(px(frame.size.width as f32), px(frame.size.height as f32)),
        )
    }
}
