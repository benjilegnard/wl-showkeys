use cairo::Context;
use pango::{AttrList, Attribute, FontDescription, Layout};
use pangocairo::functions::{create_layout, show_layout, update_layout};

pub fn get_pango_layout(cairo: &Context, font: &str, text: &str, scale: f64) -> Layout {
    let layout = create_layout(cairo);
    let attrs = AttrList::new();

    layout.set_text(text);
    attrs.change(pango::AttrFloat::new_scale(scale));

    let desc = FontDescription::from_string(font);
    layout.set_font_description(Some(&desc));
    layout.set_single_paragraph_mode(true);
    layout.set_attributes(Some(&attrs));

    layout
}

pub fn get_text_size(
    cairo: &Context,
    font: &str,
    scale: f64,
    text: &str,
) -> (i32, i32, i32) {
    let layout = get_pango_layout(cairo, font, text, scale);
    update_layout(cairo, &layout);

    let (width, height) = layout.pixel_size();
    let baseline = layout.baseline() / pango::SCALE;

    (width, height, baseline)
}

pub fn pango_printf(cairo: &Context, font: &str, scale: f64, text: &str) {
    let layout = get_pango_layout(cairo, font, text, scale);

    if let Ok(fo) = cairo.font_options() {
        let context = layout.context();
        pangocairo::functions::context_set_font_options(&context, Some(&fo));
    }

    update_layout(cairo, &layout);
    show_layout(cairo, &layout);
}
