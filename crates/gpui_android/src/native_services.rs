//! Default Android system clipboard and external URL services for shared GPUI views.
//! Only OS services use JNI. No UI widgets or application state live here.
use android_activity::AndroidApp;
use gpui::ClipboardItem;
use jni::{
    JNIEnv, JavaVM,
    objects::{JObject, JString, JValue},
};

fn with_activity<T>(
    app: &AndroidApp,
    operation: impl FnOnce(&mut JNIEnv<'_>, &JObject<'_>) -> jni::errors::Result<T>,
) -> Option<T> {
    // AndroidApp retains the VM and borrowed Activity reference throughout
    // this synchronous operation. JObject does not own/delete that reference.
    let vm = unsafe { JavaVM::from_raw(app.vm_as_ptr().cast()) }.ok()?;
    let mut env = vm.attach_current_thread().ok()?;
    let activity = unsafe { JObject::from_raw(app.activity_as_ptr().cast()) };
    let result = env.with_local_frame(32, |env| operation(env, &activity));
    if env.exception_check().unwrap_or(true) {
        // Exceptions can include user clipboard/URL content. Never describe
        // them to logcat. Clear the exception before returning to GPUI.
        let _ = env.exception_clear();
        log::warn!("Android system service rejected an operation");
        return None;
    }
    result.ok()
}

fn clipboard<'a>(env: &mut JNIEnv<'a>, activity: &JObject<'_>) -> jni::errors::Result<JObject<'a>> {
    let name = env.new_string("clipboard")?;
    env.call_method(
        activity,
        "getSystemService",
        "(Ljava/lang/String;)Ljava/lang/Object;",
        &[JValue::Object(name.as_ref())],
    )?
    .l()
}

pub(crate) fn read_clipboard(app: &AndroidApp) -> Option<ClipboardItem> {
    with_activity(app, |env, activity| {
        let manager = clipboard(env, activity)?;
        if manager.is_null() {
            return Ok(None);
        }
        let clip = env
            .call_method(
                &manager,
                "getPrimaryClip",
                "()Landroid/content/ClipData;",
                &[],
            )?
            .l()?;
        if clip.is_null() {
            return Ok(None);
        }
        let count = env.call_method(&clip, "getItemCount", "()I", &[])?.i()?;
        if count <= 0 {
            return Ok(None);
        }
        let item = env
            .call_method(
                &clip,
                "getItemAt",
                "(I)Landroid/content/ClipData$Item;",
                &[JValue::Int(0)],
            )?
            .l()?;
        // getText avoids silently resolving content-provider URIs, which
        // could perform network or permission-sensitive reads on the UI loop.
        let text = env
            .call_method(&item, "getText", "()Ljava/lang/CharSequence;", &[])?
            .l()?;
        if text.is_null() {
            return Ok(None);
        }
        let text = env
            .call_method(&text, "toString", "()Ljava/lang/String;", &[])?
            .l()?;
        let text: String = env.get_string(&JString::from(text))?.into();
        Ok(Some(ClipboardItem::new_string(text)))
    })
    .flatten()
}

pub(crate) fn write_clipboard(app: &AndroidApp, item: ClipboardItem) {
    let Some(text) = item.text() else { return };
    with_activity(app, |env, activity| {
        let manager = clipboard(env, activity)?;
        if manager.is_null() {
            return Ok(());
        }
        let label = env.new_string("Jcode")?;
        let text = env.new_string(text)?;
        let clip = env
            .call_static_method(
                "android/content/ClipData",
                "newPlainText",
                "(Ljava/lang/CharSequence;Ljava/lang/CharSequence;)Landroid/content/ClipData;",
                &[
                    JValue::Object(label.as_ref()),
                    JValue::Object(text.as_ref()),
                ],
            )?
            .l()?;
        env.call_method(
            &manager,
            "setPrimaryClip",
            "(Landroid/content/ClipData;)V",
            &[JValue::Object(&clip)],
        )?;
        Ok(())
    });
}

pub(crate) fn open_url(app: &AndroidApp, url: &str) {
    // Accept the same intentional external URL schemes as the app's links.
    // Never execute file:, javascript:, intent:, or content: from a message.
    let Some((scheme, _)) = url.split_once(':') else {
        return;
    };
    if !matches!(
        scheme.to_ascii_lowercase().as_str(),
        "https" | "http" | "mailto" | "tel" | "jcode"
    ) {
        log::warn!("Android external URL has an unsupported scheme");
        return;
    }
    with_activity(app, |env, activity| {
        let action = env.new_string("android.intent.action.VIEW")?;
        let value = env.new_string(url)?;
        let uri = env
            .call_static_method(
                "android/net/Uri",
                "parse",
                "(Ljava/lang/String;)Landroid/net/Uri;",
                &[JValue::Object(value.as_ref())],
            )?
            .l()?;
        let intent = env.new_object(
            "android/content/Intent",
            "(Ljava/lang/String;Landroid/net/Uri;)V",
            &[JValue::Object(action.as_ref()), JValue::Object(&uri)],
        )?;
        env.call_method(
            activity,
            "startActivity",
            "(Landroid/content/Intent;)V",
            &[JValue::Object(&intent)],
        )?;
        Ok(())
    });
}
