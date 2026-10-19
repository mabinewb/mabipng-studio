use anyhow::Result;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Algorithm {
    #[default]
    NeuQuant,
    Wu,
    Kmeans,
    ExoQuant,
}

impl Algorithm {
    pub const ALL: [Self; 4] = [Self::NeuQuant, Self::Wu, Self::Kmeans, Self::ExoQuant];

    pub fn label(self) -> &'static str {
        match self {
            Self::NeuQuant => "NeuQuant · 기존 방식",
            Self::Wu => "Wu · Oklab",
            Self::Kmeans => "K-means · Oklab",
            Self::ExoQuant => "ExoQuant",
        }
    }
}

pub(crate) fn generate(
    samples: &[[u8; 3]],
    count: usize,
    algorithm: Algorithm,
) -> Result<Vec<[u8; 3]>> {
    match algorithm {
        Algorithm::Wu | Algorithm::Kmeans => {
            let colors = samples
                .iter()
                .map(|c| palette::Srgb::new(c[0], c[1], c[2]))
                .collect::<Vec<_>>();
            let method = if algorithm == Algorithm::Wu {
                quantette::QuantizeMethod::Wu
            } else {
                quantette::QuantizeMethod::kmeans()
            };
            let result = quantette::Pipeline::new()
                .palette_size((count as u16).try_into()?)
                .ditherer(None)
                .quantize_method(method)
                .input_slice(&colors)?
                .output_srgb8_palette();
            Ok(result
                .into_vec()
                .into_iter()
                .map(|c| [c.red, c.green, c.blue])
                .collect())
        }
        Algorithm::ExoQuant => {
            let histogram = samples
                .iter()
                .map(|c| exoquant::Color::new(c[0], c[1], c[2], 255))
                .collect();
            Ok(exoquant::generate_palette(
                &histogram,
                &exoquant::SimpleColorSpace::default(),
                &exoquant::optimizer::KMeans,
                count,
            )
            .into_iter()
            .map(|c| [c.r, c.g, c.b])
            .collect())
        }
        Algorithm::NeuQuant => unreachable!("legacy quantizer is handled by engine"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::{self, Document, Settings};

    #[test]
    fn all_algorithms_produce_valid_distinct_four_color_chats() {
        let rgba = (0..128 * 48)
            .flat_map(|i| {
                let x = i % 128;
                let y = i / 128;
                [
                    (x * 2) as u8,
                    (y * 5) as u8,
                    ((x * 3 + y * 7) % 256) as u8,
                    255,
                ]
            })
            .collect::<Vec<_>>();
        let source = engine::document_from_rgba(128, 48, &rgba).unwrap();
        let mut outputs = std::collections::BTreeSet::new();
        for algorithm in Algorithm::ALL {
            let start = std::time::Instant::now();
            let settings = Settings {
                algorithm,
                author: Some("Test".into()),
                fix_palette: false,
                preserve_white_background: false,
                ..Default::default()
            };
            let output =
                Document::from_bytes(engine::process(&source, &settings).unwrap()).unwrap();
            assert!(output.is_chat());
            assert_eq!(output.image.dimensions(), (128, 48));
            assert!(output.palette.len() <= 4 && !output.palette.is_empty());
            assert!(engine::validate_game_size(&output).is_ok());
            outputs.insert(output.image.into_raw());
            eprintln!("{algorithm:?}: {:?}", start.elapsed());
        }
        assert!(
            outputs.len() >= 3,
            "algorithms should offer genuinely different results"
        );
    }

    #[test]
    fn every_algorithm_retains_one_to_four_exact_input_colors_without_corrections() {
        for count in 1..=4 {
            let rgba = (0..128 * 2)
                .flat_map(|i| {
                    let v = ((i % count) * 60) as u8;
                    [v, v, v, 255]
                })
                .collect::<Vec<_>>();
            let source = engine::document_from_rgba(128, 2, &rgba).unwrap();
            for algorithm in Algorithm::ALL {
                let settings = Settings {
                    algorithm,
                    author: Some("Test".into()),
                    fix_palette: false,
                    preserve_white_background: false,
                    ..Default::default()
                };
                let output =
                    Document::from_bytes(engine::process(&source, &settings).unwrap()).unwrap();
                assert_eq!(output.image.as_raw(), &rgba);
            }
        }
    }
}
