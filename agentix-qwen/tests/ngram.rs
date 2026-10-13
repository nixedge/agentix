use agentix_qwen::{
    ngram::{Config, Ngram},
    Result,
};

// Fixed integer vectors calculated with separate BigInt arithmetic from the pinned
// reference equations (not outputs captured from this Rust implementation).
const GOLDEN: [[u64; 16]; 6] = [
    [
        16121432, 28938500, 59087997, 73487090, 81148277, 104500129, 120276032, 149373875,
        176283436, 184305849, 216528839, 231080079, 257961536, 266068568, 289043455, 305959965,
    ],
    [
        15220011, 32170723, 40646129, 76511807, 98682440, 106072663, 127158041, 141938523,
        170367010, 180520041, 210849650, 234005644, 252857364, 272068622, 291885712, 311569748,
    ],
    [
        2993698, 22555595, 46746235, 65551934, 83400007, 116956842, 138173940, 148109607,
        165238550, 195531093, 219556029, 234233279, 250491792, 267808615, 280482789, 305617187,
    ],
    [
        5318554, 26730617, 51177670, 61593418, 94411889, 119482836, 127837362, 140373362,
        175277381, 199452204, 217186100, 238911097, 253868456, 270051525, 282384981, 306094789,
    ],
    [
        11040574, 24501192, 51231614, 76654297, 92730965, 111423247, 139461694, 156846261,
        172354500, 199909878, 213875332, 221704937, 259257811, 277170064, 283220709, 314073144,
    ],
    [
        8807125, 30450454, 51272170, 64422588, 81408710, 113737508, 122230591, 146888097,
        171649727, 195298022, 212887723, 231715632, 253540006, 275364398, 289305919, 319013232,
    ],
];

#[test]
fn pinned_hash_vectors_chunking_and_eos() -> Result<()> {
    let tokens = [1, 2, 248_044, 3, 4, 5];
    let mut all = Ngram::new(Config::flash_next())?;
    assert_eq!(all.total_rows(), 320_001_446);
    let output = all.extend(&tokens)?;
    for (actual, expected) in output.iter().zip(GOLDEN) {
        assert_eq!(actual.as_slice(), expected);
    }
    for split in 0..=tokens.len() {
        let mut chunked = Ngram::new(Config::flash_next())?;
        let mut actual = chunked.extend(&tokens[..split])?;
        actual.extend(chunked.extend(&tokens[split..])?);
        assert_eq!(actual, output);
    }
    let mut fresh = Ngram::new(Config::flash_next())?;
    assert_eq!(fresh.push(3)?, output[3]);
    Ok(())
}

#[test]
fn snapshot_and_rejected_batch_preserve_history() -> Result<()> {
    let mut ngram = Ngram::new(Config::flash_next())?;
    ngram.extend(&[1, 2])?;
    let snapshot = ngram.snapshot();
    let expected = ngram.extend(&[3, 4, 5])?;
    ngram.restore(&snapshot)?;
    assert!(ngram.extend(&[99, 248_320]).is_err());
    assert_eq!(ngram.extend(&[3, 4, 5])?, expected);
    ngram.restore(&snapshot)?;
    assert!(ngram.push(u32::MAX).is_err());
    assert_eq!(ngram.extend(&[3, 4, 5])?, expected);
    let mut other = Config::flash_next();
    other.seed += 1;
    assert!(ngram.restore(&Ngram::new(other)?.snapshot()).is_err());
    ngram.reset();
    assert_eq!(ngram.push(1)?, GOLDEN[0]);
    Ok(())
}

#[test]
fn invalid_geometry_is_rejected() {
    let mut config = Config::flash_next();
    config.size = 1;
    assert!(Ngram::new(config).is_err());
    let mut config = Config::flash_next();
    config.eos = config.vocabulary;
    assert!(Ngram::new(config).is_err());
}

#[test]
fn every_token_addresses_only_logical_ple_rows() -> Result<()> {
    let config = Config::flash_next();
    let vocabulary = config.vocabulary;
    let mut ngram = Ngram::new(config)?;
    let logical_rows = ngram.total_rows();
    assert_eq!(logical_rows, 320_001_446);

    for token in 0..vocabulary {
        ngram.push_with(token, |row_ids| {
            assert_eq!(row_ids.len(), 16);
            assert!(row_ids.iter().all(|&row_id| row_id < logical_rows));
            Ok(())
        })?;
    }
    Ok(())
}
