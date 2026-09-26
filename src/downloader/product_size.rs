use std::collections::BTreeMap;

use crate::depot::Depot;

/// The size of one product (the base game or a DLC) in a build, as returned by
/// [`GogDl::get_product_sizes`](crate::GogDl::get_product_sizes).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProductSize {
    /// The product's ID, as a string, like [`ProductBundle::product_id`](crate::ProductBundle::product_id).
    pub product_id: String,
    /// The product's size on disk once installed, in bytes: the sum of its depots' sizes.
    pub size: u64,
    /// The bytes the download transfers, compressed: the sum of its depots' compressed sizes.
    pub compressed_size: u64,
}

/// Sums `depots` per product, ordered by `product_id` so the result doesn't depend on hash order.
pub(crate) fn sum_by_product(depots: &[Depot]) -> Vec<ProductSize> {
    let mut sums: BTreeMap<&str, (u64, u64)> = BTreeMap::new();
    for depot in depots {
        let entry = sums.entry(&depot.product_id).or_default();
        entry.0 += depot.size;
        entry.1 += depot.compressed_size;
    }
    sums.into_iter()
        .map(|(product_id, (size, compressed_size))| ProductSize {
            product_id: product_id.to_string(),
            size,
            compressed_size,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::depot::BuildMetadata;

    fn depot(product_id: &str, size: u64, compressed_size: u64) -> Depot {
        Depot {
            manifest: String::new(),
            size,
            compressed_size,
            product_id: product_id.to_string(),
            languages: vec!["*".to_string()],
        }
    }

    #[test]
    fn depots_of_one_product_sum_and_the_result_is_sorted() {
        let depots = [depot("20", 5, 3), depot("10", 1, 1), depot("20", 7, 4)];
        assert_eq!(
            sum_by_product(&depots),
            vec![
                ProductSize {
                    product_id: "10".into(),
                    size: 1,
                    compressed_size: 1
                },
                ProductSize {
                    product_id: "20".into(),
                    size: 12,
                    compressed_size: 7
                },
            ]
        );
        assert!(sum_by_product(&[]).is_empty());
    }

    /// A captured build (a game with two DLCs): the sums cover the depots the download would use,
    /// so the per-language depots outside `en-US`/`en` are left out and the `*` depot is counted.
    #[test]
    fn a_captured_build_sums_per_product_after_the_language_filter() {
        let mut metadata: BuildMetadata =
            serde_json::from_str(include_str!("../../tests/fixtures/build_metadata_dlc.json"))
                .unwrap();
        assert_eq!(metadata.depots.len(), 90);
        metadata.filter_languages(&["en-US", "en"]);
        assert_eq!(metadata.depots.len(), 10);

        let sizes = sum_by_product(&metadata.depots);
        let sizes: Vec<_> = sizes
            .iter()
            .map(|s| (s.product_id.as_str(), s.size, s.compressed_size))
            .collect();
        assert_eq!(
            sizes,
            vec![
                ("1256837418", 24_864_943_837, 24_405_201_081),
                ("1423049311", 66_380_040_042, 64_306_413_665),
                ("1597316373", 97_227_175, 24_947_062),
            ]
        );
    }
}
