//! Read and format the metadata shown in the info panel.

use std::fs::File;
use std::io::BufReader;
use std::path::Path;

use exif::{In, Reader, Tag, Value};

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
    }
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
    }
}
