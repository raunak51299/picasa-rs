//! Read and format the metadata shown in the info panel.

use std::collections::{BTreeMap, HashMap};
use std::fs::File;
use std::io::BufReader;
use std::path::Path;

use exif::{Context, Exif, Field, In, Reader, Tag, Value};

const MAX_BINARY_BYTES: usize = 64;
const MAX_VALUE_CHARS: usize = 240;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MetadataEntry {
    pub tag: String,
    pub value: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MetadataSection {
    pub label: String,
    pub entries: Vec<MetadataEntry>,
}

#[derive(Debug, Default)]
pub struct PhotoInfo {
    pub file_name: String,
    pub dimensions: Option<(u32, u32)>,
    pub file_size: Option<u64>,
    pub date: Option<String>,
    pub camera: Option<String>,
    pub lens: Option<String>,
    pub exposure: Option<String>,
    pub aperture: Option<String>,
    pub iso: Option<u32>,
    pub focal_length: Option<String>,
    pub gps: Option<String>,
    /// Every EXIF field, grouped by IFD and tag context.
    pub metadata: Vec<MetadataSection>,
}

pub fn read(path: &Path) -> PhotoInfo {
    let file_name = path
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();
    let file_size = std::fs::metadata(path).ok().map(|metadata| metadata.len());

    let exif = File::open(path).ok().and_then(|file| {
        Reader::new()
            .read_from_container(&mut BufReader::new(file))
            .ok()
    });

    let orientation = exif
        .as_ref()
        .and_then(|exif| exif.get_field(Tag::Orientation, In::PRIMARY))
        .and_then(|field| field.value.get_uint(0))
        .unwrap_or(1);
    let dimensions = header_dimensions(path)
        .or_else(|| {
            exif.as_ref().and_then(|exif| {
                let width = exif
                    .get_field(Tag::PixelXDimension, In::PRIMARY)
                    .and_then(|field| field.value.get_uint(0))?;
                let height = exif
                    .get_field(Tag::PixelYDimension, In::PRIMARY)
                    .and_then(|field| field.value.get_uint(0))?;
                Some((width, height))
            })
        })
        .map(|(width, height)| {
            if orientation >= 5 {
                (height, width)
            } else {
                (width, height)
            }
        });

    let Some(exif) = exif else {
        return PhotoInfo {
            file_name,
            dimensions,
            file_size,
            ..PhotoInfo::default()
        };
    };

    let ascii = |tag: Tag| {
        exif.get_field(tag, In::PRIMARY)
            .and_then(|field| match &field.value {
                Value::Ascii(values) => values.first().map(|bytes| {
                    String::from_utf8_lossy(bytes)
                        .trim_end_matches('\0')
                        .trim()
                        .to_string()
                }),
                _ => None,
            })
            .filter(|text| !text.is_empty())
    };
    let rational = |tag: Tag| {
        exif.get_field(tag, In::PRIMARY)
            .and_then(|field| match &field.value {
                Value::Rational(values) => values.first().map(|value| value.to_f64()),
                _ => None,
            })
            .filter(|value| value.is_finite())
    };

    let make = ascii(Tag::Make);
    let model = ascii(Tag::Model);
    let camera = match (make, model) {
        (Some(make), Some(model)) => {
            if model.to_lowercase().starts_with(&make.to_lowercase()) {
                Some(model)
            } else {
                Some(format!("{make} {model}"))
            }
        }
        (None, model) => model,
        (make, None) => make,
    };

    let date = ascii(Tag::DateTimeOriginal)
        .or_else(|| ascii(Tag::DateTimeDigitized))
        .or_else(|| ascii(Tag::DateTime))
        .and_then(|raw| format_exif_date(&raw));

    let exposure = rational(Tag::ExposureTime)
        .filter(|seconds| *seconds > 0.0)
        .map(format_exposure);
    let aperture = rational(Tag::FNumber).map(|number| format!("f/{number:.1}"));
    let iso = exif
        .get_field(Tag::PhotographicSensitivity, In::PRIMARY)
        .and_then(|field| field.value.get_uint(0));
    let focal_length = rational(Tag::FocalLength).map(|mm| {
        let equivalent = exif
            .get_field(Tag::FocalLengthIn35mmFilm, In::PRIMARY)
            .and_then(|field| field.value.get_uint(0))
            .filter(|value| *value > 0);
        match equivalent {
            Some(equivalent) => format!("{} mm ({} mm equiv.)", format_mm(mm), equivalent),
            None => format!("{} mm", format_mm(mm)),
        }
    });

    let gps = gps_string(&exif);
    let metadata = collect_metadata(&exif);

    PhotoInfo {
        file_name,
        dimensions,
        file_size,
        date,
        camera,
        lens: ascii(Tag::LensModel),
        exposure,
        aperture,
        iso,
        focal_length,
        gps,
        metadata,
    }
}

#[derive(Default)]
struct MetadataSectionBuilder {
    entries: Vec<MetadataEntry>,
    by_tag: HashMap<Tag, usize>,
}

impl MetadataSectionBuilder {
    fn push(&mut self, tag: Tag, label: String, value: String) {
        if let Some(&index) = self.by_tag.get(&tag) {
            self.entries[index] = MetadataEntry { tag: label, value };
        } else {
            self.by_tag.insert(tag, self.entries.len());
            self.entries.push(MetadataEntry { tag: label, value });
        }
    }
}

fn collect_metadata(exif: &Exif) -> Vec<MetadataSection> {
    let mut sections: BTreeMap<(In, Context), MetadataSectionBuilder> = BTreeMap::new();
    for field in exif.fields() {
        sections
            .entry((field.ifd_num, field.tag.context()))
            .or_default()
            .push(field.tag, tag_label(field.tag), value_text(exif, field));
    }
    sections
        .into_iter()
        .map(|((ifd, context), section)| MetadataSection {
            label: section_label(ifd, context),
            entries: section.entries,
        })
        .collect()
}

fn section_label(ifd: In, context: Context) -> String {
    let base = match ifd.index() {
        0 => "Image".to_string(),
        1 => "Thumbnail".to_string(),
        index => format!("IFD {index}"),
    };
    let suffix = match context {
        Context::Tiff => return base,
        Context::Exif => "EXIF",
        Context::Gps => "GPS",
        Context::Interop => "Interoperability",
        _ => "Metadata",
    };
    if ifd.index() == 0 {
        suffix.to_string()
    } else {
        format!("{base} {suffix}")
    }
}

fn tag_label(tag: Tag) -> String {
    if tag.description().is_some() {
        tag.to_string()
    } else {
        format!("Tag 0x{:04X}", tag.number())
    }
}

fn value_text(exif: &Exif, field: &Field) -> String {
    if let Some(comment) = user_comment_text(field) {
        return truncate_chars(&comment, MAX_VALUE_CHARS);
    }
    match binary_summary(&field.value) {
        Some(summary) => summary,
        None => truncate_chars(
            &field.display_value().with_unit(exif).to_string(),
            MAX_VALUE_CHARS,
        ),
    }
}

fn user_comment_text(field: &Field) -> Option<String> {
    if field.tag != Tag::UserComment {
        return None;
    }
    let bytes = match &field.value {
        Value::Undefined(bytes, _) | Value::Byte(bytes) => bytes.as_slice(),
        _ => return None,
    };
    let text = bytes.strip_prefix(b"ASCII\0\0\0")?;
    let text = String::from_utf8_lossy(text)
        .trim_end_matches('\0')
        .trim()
        .to_string();
    (!text.is_empty()).then_some(text)
}

fn binary_summary(value: &Value) -> Option<String> {
    let length = match value {
        Value::Undefined(bytes, _) | Value::Byte(bytes) => bytes.len(),
        _ => return None,
    };
    (length > MAX_BINARY_BYTES).then(|| format!("{length} bytes of binary data"))
}

fn truncate_chars(text: &str, max: usize) -> String {
    let mut chars = text.chars();
    let mut result: String = chars.by_ref().take(max).collect();
    if chars.next().is_some() {
        result.push('…');
    }
    result
}

fn header_dimensions(path: &Path) -> Option<(u32, u32)> {
    image::ImageReader::open(path)
        .ok()?
        .with_guessed_format()
        .ok()?
        .into_dimensions()
        .ok()
}

fn gps_string(exif: &exif::Exif) -> Option<String> {
    let coordinate = |tag: Tag, reference_tag: Tag| {
        let values = match &exif.get_field(tag, In::PRIMARY)?.value {
            Value::Rational(values) if values.len() >= 3 => values,
            _ => return None,
        };
        let decimal = dms_to_decimal(values)?;
        let reference = exif
            .get_field(reference_tag, In::PRIMARY)
            .and_then(|field| match &field.value {
                Value::Ascii(values) => values.first().map(|bytes| {
                    String::from_utf8_lossy(bytes)
                        .trim_end_matches('\0')
                        .trim()
                        .to_uppercase()
                }),
                _ => None,
            })
            .unwrap_or_default();
        let signed = if reference == "S" || reference == "W" {
            -decimal
        } else {
            decimal
        };
        Some((signed, reference))
    };

    let (latitude, latitude_ref) = coordinate(Tag::GPSLatitude, Tag::GPSLatitudeRef)?;
    let (longitude, longitude_ref) = coordinate(Tag::GPSLongitude, Tag::GPSLongitudeRef)?;
    Some(format!(
        "{:.6}° {latitude_ref}, {:.6}° {longitude_ref}",
        latitude.abs(),
        longitude.abs()
    ))
}

pub fn dms_to_decimal(values: &[exif::Rational]) -> Option<f64> {
    if values.len() < 3 {
        return None;
    }
    let degrees = values[0].to_f64();
    let minutes = values[1].to_f64();
    let seconds = values[2].to_f64();
    if !degrees.is_finite() || !minutes.is_finite() || !seconds.is_finite() {
        return None;
    }
    Some(degrees + minutes / 60.0 + seconds / 3600.0)
}

pub fn format_exposure(seconds: f64) -> String {
    if seconds >= 1.0 {
        if (seconds - seconds.round()).abs() < 0.05 {
            format!("{seconds:.0} s")
        } else {
            format!("{seconds:.1} s")
        }
    } else {
        let denominator = (1.0 / seconds).round().max(1.0);
        format!("1/{denominator:.0} s")
    }
}

fn format_mm(mm: f64) -> String {
    if (mm - mm.round()).abs() < 0.05 {
        format!("{mm:.0}")
    } else {
        format!("{mm:.1}")
    }
}

pub fn format_file_size(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = KB * 1024.0;
    const GB: f64 = MB * 1024.0;
    let bytes = bytes as f64;
    if bytes >= GB {
        format!("{:.2} GB", bytes / GB)
    } else if bytes >= MB {
        format!("{:.1} MB", bytes / MB)
    } else if bytes >= KB {
        format!("{:.0} KB", bytes / KB)
    } else {
        format!("{bytes:.0} B")
    }
}

/// `2024:05:17 14:03:21` becomes `2024-05-17 14:03`.
pub fn format_exif_date(raw: &str) -> Option<String> {
    let raw = raw.trim();
    let (date, time) = raw.split_once(' ')?;
    let mut date_parts = date.split(':');
    let year = date_parts.next()?;
    let month = date_parts.next()?;
    let day = date_parts.next()?;
    if date_parts.next().is_some()
        || year.len() != 4
        || month.len() != 2
        || day.len() != 2
        || !year.chars().all(|c| c.is_ascii_digit())
        || !month.chars().all(|c| c.is_ascii_digit())
        || !day.chars().all(|c| c.is_ascii_digit())
    {
        return None;
    }
    let time = time.get(..5).unwrap_or(time);
    Some(format!("{year}-{month}-{day} {time}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use exif::Rational;

    #[test]
    fn exposure_uses_fractions_below_one_second() {
        assert_eq!(format_exposure(1.0 / 125.0), "1/125 s");
        assert_eq!(format_exposure(0.5), "1/2 s");
        assert_eq!(format_exposure(2.0), "2 s");
        assert_eq!(format_exposure(2.5), "2.5 s");
    }

    #[test]
    fn gps_dms_is_converted_to_decimal_degrees() {
        let paris = [
            Rational { num: 48, denom: 1 },
            Rational { num: 51, denom: 1 },
            Rational { num: 30, denom: 1 },
        ];
        let decimal = dms_to_decimal(&paris).unwrap();
        assert!((decimal - 48.858_333).abs() < 0.001);
        assert!(dms_to_decimal(&paris[..2]).is_none());
    }

    #[test]
    fn file_sizes_use_binary_units() {
        assert_eq!(format_file_size(512), "512 B");
        assert_eq!(format_file_size(2048), "2 KB");
        assert_eq!(format_file_size(5 * 1024 * 1024), "5.0 MB");
        assert_eq!(format_file_size(3 * 1024 * 1024 * 1024), "3.00 GB");
    }

    #[test]
    fn exif_dates_are_reformatted_and_validated() {
        assert_eq!(
            format_exif_date("2024:05:17 14:03:21").as_deref(),
            Some("2024-05-17 14:03")
        );
        assert_eq!(format_exif_date("not a date"), None);
        assert_eq!(format_exif_date("2024:5:17 14:03:21"), None);
    }

    #[test]
    fn missing_metadata_is_reported_as_none() {
        let info = read(&std::env::temp_dir().join("picasa-rs-does-not-exist.jpg"));
        assert!(info.dimensions.is_none());
        assert!(info.camera.is_none());
        assert!(info.gps.is_none());
        assert!(info.metadata.is_empty());
    }

    #[test]
    fn unknown_tags_stay_visible_by_number() {
        assert_eq!(
            tag_label(Tag(Context::Tiff, 0x1234)),
            "Tag 0x1234".to_string()
        );
        assert_eq!(tag_label(Tag::Make), "Make".to_string());
    }

    #[test]
    fn metadata_sections_are_named_by_ifd_and_context() {
        assert_eq!(section_label(In::PRIMARY, Context::Tiff), "Image");
        assert_eq!(section_label(In::PRIMARY, Context::Exif), "EXIF");
        assert_eq!(section_label(In::PRIMARY, Context::Gps), "GPS");
        assert_eq!(
            section_label(In::PRIMARY, Context::Interop),
            "Interoperability"
        );
        assert_eq!(section_label(In::THUMBNAIL, Context::Tiff), "Thumbnail");
        assert_eq!(
            section_label(In::THUMBNAIL, Context::Exif),
            "Thumbnail EXIF"
        );
        assert_eq!(section_label(In(2), Context::Tiff), "IFD 2");
    }

    #[test]
    fn large_binary_values_are_summarised() {
        assert_eq!(binary_summary(&Value::Undefined(vec![0; 64], 0)), None);
        assert_eq!(
            binary_summary(&Value::Undefined(vec![0; 65], 0)).as_deref(),
            Some("65 bytes of binary data")
        );
        assert_eq!(binary_summary(&Value::Byte(vec![1; 3])), None);
    }

    #[test]
    fn long_values_are_truncated_on_a_character_boundary() {
        assert_eq!(truncate_chars("abcdef", 4), "abcd…");
        assert_eq!(truncate_chars("abcd", 4), "abcd");
        assert_eq!(truncate_chars("éééé", 2), "éé…");
    }

    #[test]
    fn ascii_user_comments_are_decoded() {
        let comment = Field {
            tag: Tag::UserComment,
            ifd_num: In::PRIMARY,
            value: Value::Byte(b"ASCII\0\0\0A short note.\0".to_vec()),
        };
        assert_eq!(
            user_comment_text(&comment).as_deref(),
            Some("A short note.")
        );

        let comment = Field {
            tag: Tag::UserComment,
            ifd_num: In::PRIMARY,
            value: Value::Undefined(b"ASCII\0\0\0A test comment".to_vec(), 0),
        };
        assert_eq!(
            user_comment_text(&comment).as_deref(),
            Some("A test comment")
        );

        let not_a_comment = Field {
            tag: Tag::Make,
            ifd_num: In::PRIMARY,
            value: Value::Byte(b"ASCII\0\0\0".to_vec()),
        };
        assert!(user_comment_text(&not_a_comment).is_none());
    }

    #[test]
    fn duplicate_tags_replace_the_earlier_value_in_place() {
        let mut section = MetadataSectionBuilder::default();
        section.push(Tag::Make, tag_label(Tag::Make), "First".to_string());
        section.push(Tag::Model, tag_label(Tag::Model), "Model".to_string());
        section.push(Tag::Make, tag_label(Tag::Make), "Second".to_string());

        let entries: Vec<_> = section
            .entries
            .iter()
            .map(|entry| (entry.tag.as_str(), entry.value.as_str()))
            .collect();
        assert_eq!(entries, vec![("Make", "Second"), ("Model", "Model")]);
    }
}
