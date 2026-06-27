use pipewire::spa::{self, sys as spa_sys};

use spa::param::format::{FormatProperties, MediaSubtype, MediaType};
use spa::param::video::VideoFormat;
use spa::param::ParamType;
use spa::pod::{Property, PropertyFlags, Value};
use spa::utils::{Choice, ChoiceEnum, ChoiceFlags, Rectangle};

pub fn connect_format_bytes() -> Vec<u8> {
    let obj = spa::pod::object!(
        spa::utils::SpaTypes::ObjectParamFormat,
        ParamType::EnumFormat,
        spa::pod::property!(
            FormatProperties::MediaType,
            Id,
            MediaType::Video
        ),
        spa::pod::property!(
            FormatProperties::MediaSubtype,
            Id,
            MediaSubtype::Raw
        ),
        spa::pod::property!(
            FormatProperties::VideoFormat,
            Choice,
            Enum,
            Id,
            VideoFormat::BGRx,
            VideoFormat::BGRx,
            VideoFormat::BGRA,
            VideoFormat::RGBA,
            VideoFormat::RGBx,
            VideoFormat::xBGR,
            VideoFormat::ARGB,
            VideoFormat::NV12,
        ),
        spa::pod::property!(
            FormatProperties::VideoSize,
            Choice,
            Range,
            Rectangle,
            Rectangle {
                width: 1,
                height: 1
            },
            Rectangle {
                width: 1,
                height: 1
            },
            Rectangle {
                width: 8192,
                height: 8192
            }
        ),
    );
    serialize_object(obj)
}

pub fn buffer_params_bytes(stride: i32, height: u32) -> Vec<u8> {
    let size = stride.saturating_mul(height as i32).max(stride);
    let obj = spa::pod::Object {
        type_: spa::utils::SpaTypes::ObjectParamBuffers.as_raw(),
        id: ParamType::Buffers.as_raw(),
        properties: vec![
            buffer_prop_int(
                spa_sys::SPA_PARAM_BUFFERS_buffers,
                ChoiceEnum::Range {
                    default: 8,
                    min: 2,
                    max: 16,
                },
            ),
            buffer_prop_int(
                spa_sys::SPA_PARAM_BUFFERS_blocks,
                ChoiceEnum::Enum {
                    default: 1,
                    alternatives: vec![1],
                },
            ),
            buffer_prop_int(
                spa_sys::SPA_PARAM_BUFFERS_size,
                ChoiceEnum::Enum {
                    default: size,
                    alternatives: vec![size],
                },
            ),
            buffer_prop_int(
                spa_sys::SPA_PARAM_BUFFERS_stride,
                ChoiceEnum::Enum {
                    default: stride,
                    alternatives: vec![stride],
                },
            ),
            buffer_prop_int(
                spa_sys::SPA_PARAM_BUFFERS_dataType,
                ChoiceEnum::Flags {
                    default: 1 << spa_sys::SPA_DATA_MemPtr,
                    flags: vec![
                        1 << spa_sys::SPA_DATA_MemPtr,
                        1 << spa_sys::SPA_DATA_MemFd,
                        1 << spa_sys::SPA_DATA_DmaBuf,
                    ],
                },
            ),
        ],
    };
    serialize_object(obj)
}

pub fn stride_for(format: VideoFormat, width: u32) -> i32 {
    let bpp = bytes_per_pixel(format);
    (width as i32).saturating_mul(bpp as i32).max(width as i32)
}

pub fn bytes_per_pixel(format: VideoFormat) -> u32 {
    if format == VideoFormat::RGB || format == VideoFormat::BGR {
        3
    } else if format == VideoFormat::NV12
        || format == VideoFormat::NV21
        || format == VideoFormat::I420
    {
        1
    } else {
        4
    }
}

fn buffer_prop_int(key: u32, choice: ChoiceEnum<i32>) -> Property {
    Property {
        key,
        flags: PropertyFlags::empty(),
        value: Value::Choice(spa::pod::ChoiceValue::Int(Choice(
            ChoiceFlags::empty(),
            choice,
        ))),
    }
}

fn serialize_object(obj: spa::pod::Object) -> Vec<u8> {
    spa::pod::serialize::PodSerializer::serialize(
        std::io::Cursor::new(Vec::new()),
        &Value::Object(obj),
    )
    .expect("serialize pod")
    .0
    .into_inner()
}
