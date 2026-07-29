//! Varint + delta encoding for sorted `doc_id` blocks.
//!
//! This module provides the first building block of the future block-128
//! FoR codec described in `docs/poc/perf-optimization-plan.md` ("Phase 2 C").
//! It is a pure, additive utility: no existing call site references it yet
//! — `TermDictionary` integration ships in a later commit on `wp/a-optim`.
//!
//! The format is intentionally minimal:
//!
//! 1. The slice length is written as an unsigned LEB128 varint.
//! 2. Each element is written as the unsigned LEB128 varint of
//!    `sorted[i] - sorted[i-1]` (with `sorted[-1] = 0`).
//!
//! Inputs are required to be **strictly increasing** `u32` doc identifiers,
//! except that the very first element MAY be `0` (delta = 0 is therefore
//! only permitted as the first written delta).

use thiserror::Error;

/// Number of postings per logical block for the future block-128 FoR
/// integration path. Kept local to `surch-codec` so the codec can be
/// exercised independently from `surch-index`.
pub const FOR_BLOCK_SIZE: usize = 128;

/// D2 : le mode patché écrit la position d'une exception sur UN octet, et
/// le nombre d'exceptions d'un bloc sur un autre. Les deux ne tiennent que
/// parce qu'un bloc porte au plus [`FOR_BLOCK_SIZE`] postings. L'invariant
/// est implicite dans `encode_block_into` ; il est figé ici pour qu'un
/// futur agrandissement de la taille de bloc échoue à la COMPILATION au
/// lieu de tronquer silencieusement une position.
const _: () = assert!(FOR_BLOCK_SIZE <= 256);

/// Number of blocks summarised by one entry in the top-level skip layer
/// of [`BlockSkipList`]. Tuned to keep the top layer dense enough to fit
/// in a 32-byte L1 cache line for `~1024` blocks (i.e. up to ~131 k
/// postings, which matches the TREC-COVID 171 k corpus order of
/// magnitude). For smaller posting lists the top layer collapses to a
/// single entry and the cursor falls back to a linear-then-binary scan
/// over the per-block layer, which is already O(log N) with `N` ≤ 16.
pub const SKIP_INTERVAL: usize = 8;

/// Errors returned by [`decode_doc_ids_delta_varint`] and
/// [`encode_doc_ids_delta_varint`].
///
/// The variants mirror the style of [`crate::codec_util::CodecUtilError`]
/// (Lucene-flavoured `thiserror` enum, `Clone + PartialEq + Eq`) so that
/// upstream error plumbing in `surch-store` / `surch-index` can treat the
/// two consistently.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum PostingsBlockError {
    /// Encountered end of input before a varint or the announced number of
    /// deltas could be fully read.
    #[error("postings block: unexpected end of input")]
    UnexpectedEof,
    /// A varint occupied more than the 5 bytes required to hold a `u32`,
    /// or its bits would not fit in `u32` after shifting.
    #[error("postings block: varint overflow (does not fit in u32)")]
    VarintOverflow,
    /// Two consecutive doc ids would not be strictly increasing once
    /// reconstructed: either a `0` delta past the first element, or an
    /// overflow when accumulating.
    #[error("postings block: doc ids are not strictly monotonic")]
    NotMonotonic,
    /// Octets qu'un encodeur canonique n'aurait jamais pu produire : bit
    /// réservé de l'en-tête posé, mode de canal inconnu, largeur de
    /// bit-packing nulle ou supérieure à 32, liste d'exceptions
    /// incohérente, ou constante de fréquence égale à 1 (elle aurait été
    /// écrite dans le mode « toutes à 1 », sans octet). Distincte de
    /// [`Self::NotMonotonic`] : ici c'est la FORME du bloc qui est
    /// invalide, pas la suite d'identifiants qu'il reconstruirait.
    #[error("postings block: malformed block header or channel")]
    MalformedBlock,
}

/// Lightweight metadata describing one logical block inside the payload
/// produced by [`encode_postings_doc_id_freq`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PostingsBlockMeta {
    pub block_index: usize,
    pub posting_count: usize,
    pub min_doc_id: u32,
    pub max_doc_id: u32,
    pub max_term_freq: u32,
    pub doc_ids_byte_start: usize,
    pub doc_ids_byte_end: usize,
    pub freqs_byte_start: usize,
    pub freqs_byte_end: usize,
}

/// Encode `sorted` (a strictly increasing slice of `u32` doc ids) as a
/// length-prefixed sequence of delta-varint values.
///
/// Returns [`PostingsBlockError::NotMonotonic`] if `sorted` is not strictly
/// increasing. An empty input encodes to a single byte (`0x00`, the varint
/// for length `0`).
pub fn encode_doc_ids_delta_varint(sorted: &[u32]) -> Result<Vec<u8>, PostingsBlockError> {
    validate_strictly_increasing(sorted)?;

    let mut out = Vec::with_capacity(sorted.len() + 1);
    write_varint_u32(&mut out, sorted.len() as u32);

    let mut previous: u32 = 0;
    for &value in sorted {
        let delta = value - previous; // safe: validate_* above guarantees value >= previous.
        write_varint_u32(&mut out, delta);
        previous = value;
    }
    Ok(out)
}

/// Decode the output of [`encode_doc_ids_delta_varint`] back into the
/// original `Vec<u32>` of strictly increasing doc ids.
pub fn decode_doc_ids_delta_varint(bytes: &[u8]) -> Result<Vec<u32>, PostingsBlockError> {
    let mut position = 0;
    let length = read_varint_u32(bytes, &mut position)? as usize;

    let mut out = Vec::with_capacity(length);
    let mut previous: u32 = 0;
    for i in 0..length {
        let delta = read_varint_u32(bytes, &mut position)?;
        if i > 0 && delta == 0 {
            return Err(PostingsBlockError::NotMonotonic);
        }
        let value = previous
            .checked_add(delta)
            .ok_or(PostingsBlockError::NotMonotonic)?;
        out.push(value);
        previous = value;
    }
    Ok(out)
}

/// Convenience helper used by tests: encode then decode and return the
/// reconstructed vector. Panics on encode/decode errors — callers in
/// production code should use the two functions above directly.
#[doc(hidden)]
pub fn round_trip(sorted: &[u32]) -> Vec<u32> {
    let encoded = encode_doc_ids_delta_varint(sorted).expect("encode failed");
    decode_doc_ids_delta_varint(&encoded).expect("decode failed")
}

fn validate_strictly_increasing(sorted: &[u32]) -> Result<(), PostingsBlockError> {
    for window in sorted.windows(2) {
        if window[0] >= window[1] {
            return Err(PostingsBlockError::NotMonotonic);
        }
    }
    Ok(())
}

/// Write `value` as an unsigned LEB128 varint (1..=5 bytes for any `u32`).
fn write_varint_u32(out: &mut Vec<u8>, mut value: u32) {
    while value >= 0x80 {
        out.push(((value & 0x7f) as u8) | 0x80);
        value >>= 7;
    }
    out.push(value as u8);
}

/// Read an unsigned LEB128 varint. Returns [`PostingsBlockError::UnexpectedEof`]
/// if input is exhausted before the terminating byte, and
/// [`PostingsBlockError::VarintOverflow`] if the encoded value would not fit
/// in a `u32`.
fn read_varint_u32(input: &[u8], position: &mut usize) -> Result<u32, PostingsBlockError> {
    let mut result: u32 = 0;
    let mut shift: u32 = 0;
    loop {
        let byte = *input
            .get(*position)
            .ok_or(PostingsBlockError::UnexpectedEof)?;
        *position += 1;

        let chunk = u32::from(byte & 0x7f);
        // After 4 full 7-bit groups (shift = 28) only the low 4 bits of the
        // 5th byte may be set, otherwise the value would not fit in u32.
        if shift == 28 && byte & 0x80 != 0 {
            return Err(PostingsBlockError::VarintOverflow);
        }
        if shift == 28 && chunk > 0x0f {
            return Err(PostingsBlockError::VarintOverflow);
        }
        if shift >= 32 {
            return Err(PostingsBlockError::VarintOverflow);
        }

        result |= chunk << shift;

        if byte & 0x80 == 0 {
            return Ok(result);
        }
        shift += 7;
    }
}

/// Encode a parallel pair of slices `(doc_ids, freqs)` as a compact
/// `Vec<u8>` payload suitable for cold-tier posting list storage.
///
/// The layout is intentionally simple — it is the building block of
/// the future block-128 FoR codec described in
/// `docs/poc/for-integration-plan.md` (Phase 1):
///
/// 1. The number of postings `n` is written as a varint.
/// 2. `n` delta-varint doc ids follow (same scheme as
///    [`encode_doc_ids_delta_varint`]).
/// 3. `n` raw varint frequencies follow (no delta — frequencies are
///    not monotonic; varint already compresses the typical 1..16 range
///    to a single byte).
///
/// `doc_ids` and `freqs` must have the same length, otherwise
/// [`PostingsBlockError::NotMonotonic`] is returned (the variant name
/// is reused to avoid a breaking enum change — see the integration
/// plan for the dedicated `LengthMismatch` variant scheduled in Phase
/// 2).
pub fn encode_postings_doc_id_freq(
    doc_ids: &[u32],
    freqs: &[u32],
) -> Result<Vec<u8>, PostingsBlockError> {
    if doc_ids.len() != freqs.len() {
        return Err(PostingsBlockError::NotMonotonic);
    }
    validate_strictly_increasing(doc_ids)?;

    // Rough capacity heuristic: 1 byte length header + ~1.5 bytes per
    // delta + ~1 byte per freq. Avoids the first 1–2 grow cycles of
    // `Vec::push` on the BAN 25 k corpus (mean term length ≈ 8 postings).
    let mut out = Vec::with_capacity(1 + doc_ids.len() * 3);
    write_varint_u32(&mut out, doc_ids.len() as u32);

    let mut previous: u32 = 0;
    for &value in doc_ids {
        let delta = value - previous;
        write_varint_u32(&mut out, delta);
        previous = value;
    }
    for &freq in freqs {
        write_varint_u32(&mut out, freq);
    }
    Ok(out)
}

/// Decode the output of [`encode_postings_doc_id_freq`] back into the
/// two parallel `Vec<u32>` slices.
pub fn decode_postings_doc_id_freq(
    bytes: &[u8],
) -> Result<(Vec<u32>, Vec<u32>), PostingsBlockError> {
    let mut position = 0;
    let length = read_varint_u32(bytes, &mut position)? as usize;

    let mut doc_ids = Vec::with_capacity(length);
    let mut previous: u32 = 0;
    for i in 0..length {
        let delta = read_varint_u32(bytes, &mut position)?;
        if i > 0 && delta == 0 {
            return Err(PostingsBlockError::NotMonotonic);
        }
        let value = previous
            .checked_add(delta)
            .ok_or(PostingsBlockError::NotMonotonic)?;
        doc_ids.push(value);
        previous = value;
    }

    let mut freqs = Vec::with_capacity(length);
    for _ in 0..length {
        freqs.push(read_varint_u32(bytes, &mut position)?);
    }

    Ok((doc_ids, freqs))
}

/// Nombre maximal de bits d'une valeur bit-packée : les identifiants et
/// les fréquences sont des `u32`.
const MAX_PACKED_WIDTH: u32 = 32;

/// Masque du mode du canal doc_id dans l'octet d'en-tête de bloc.
const DOC_MODE_MASK: u8 = 0b0000_0011;
/// Décalage du mode du canal freq dans l'octet d'en-tête de bloc.
const FREQ_MODE_SHIFT: u32 = 2;
/// Bits réservés de l'octet d'en-tête : ils DOIVENT valoir 0. Un bit posé
/// est un octet non canonique, donc une corruption explicite.
const HEADER_RESERVED_MASK: u8 = 0b1111_0000;

/// Deltas écrits en LEB128, un varint par valeur (le format historique).
const DOC_MODE_VARINT: u8 = 0;
/// Deltas bit-packés à largeur fixe (frame of reference).
const DOC_MODE_PACKED: u8 = 1;
/// Deltas bit-packés + liste d'exceptions (patched frame of reference).
const DOC_MODE_PATCHED: u8 = 2;

/// Toutes les fréquences du bloc valent 1 : le canal est ABSENT du payload.
const FREQ_MODE_ALL_ONES: u8 = 0;
/// Toutes les fréquences du bloc sont égales à une constante != 1, écrite
/// une seule fois en varint.
const FREQ_MODE_CONSTANT: u8 = 1;
/// Fréquences non constantes : bit-packées à largeur fixe.
const FREQ_MODE_PACKED: u8 = 2;
/// Fréquences non constantes écrites en varint, une par valeur — le mode
/// historique, conservé comme REPLI. Il n'est retenu que lorsqu'il est
/// strictement moins cher que le bit-packing (fréquences très dispersées
/// dans un petit bloc) : c'est ce qui garantit que le canal `freq` n'est
/// JAMAIS plus gros qu'avant D2.
const FREQ_MODE_VARINT: u8 = 3;

/// Nombre de bits utiles de `value` (`0` pour `0`).
fn bit_width(value: u32) -> u32 {
    u32::BITS - value.leading_zeros()
}

/// Taille, en octets, du varint LEB128 d'une valeur de `bits` bits utiles.
fn varint_len_for_bits(bits: u32) -> usize {
    match bits {
        0..=7 => 1,
        8..=14 => 2,
        15..=21 => 3,
        22..=28 => 4,
        _ => 5,
    }
}

/// Taille, en octets, de `count` valeurs bit-packées sur `width` bits.
fn packed_len(count: usize, width: u32) -> usize {
    (count as u64 * u64::from(width)).div_ceil(8) as usize
}

/// Écrit `values` bit-packées sur `width` bits, poids faibles d'abord.
/// Les bits de bourrage du dernier octet sont laissés à `0` : c'est ce qui
/// rend l'encodage CANONIQUE (une seule suite d'octets par entrée).
fn pack_bits(out: &mut Vec<u8>, values: &[u32], width: u32) {
    if width == 0 || values.is_empty() {
        return;
    }
    let start = out.len();
    out.resize(start + packed_len(values.len(), width), 0);
    let mut bit_cursor: usize = 0;
    for &value in values {
        let mut remaining = width;
        let mut rest = value;
        while remaining > 0 {
            let byte_index = start + bit_cursor / 8;
            let bit_in_byte = (bit_cursor % 8) as u32;
            let take = remaining.min(8 - bit_in_byte);
            let chunk = (rest & ((1u32 << take) - 1)) as u8;
            out[byte_index] |= chunk << bit_in_byte;
            rest >>= take;
            bit_cursor += take as usize;
            remaining -= take;
        }
    }
}

/// Inverse de [`pack_bits`]. Avance `*position` du nombre exact d'octets
/// consommés et refuse une entrée tronquée plutôt que de compléter par des
/// zéros.
fn unpack_bits(
    bytes: &[u8],
    position: &mut usize,
    count: usize,
    width: u32,
    out: &mut Vec<u32>,
) -> Result<(), PostingsBlockError> {
    if count == 0 {
        return Ok(());
    }
    let byte_len = packed_len(count, width);
    let slice = bytes
        .get(*position..*position + byte_len)
        .ok_or(PostingsBlockError::UnexpectedEof)?;
    let mut bit_cursor: usize = 0;
    for _ in 0..count {
        let mut value: u32 = 0;
        let mut collected: u32 = 0;
        while collected < width {
            let byte = slice[bit_cursor / 8];
            let bit_in_byte = (bit_cursor % 8) as u32;
            let take = (width - collected).min(8 - bit_in_byte);
            let mask = ((1u16 << take) - 1) as u8;
            value |= u32::from((byte >> bit_in_byte) & mask) << collected;
            bit_cursor += take as usize;
            collected += take;
        }
        out.push(value);
    }
    *position += byte_len;
    Ok(())
}

/// Compteurs d'écriture du codec, agrégés par
/// [`encode_postings_blocked_with_directory`] et remontés jusqu'aux jauges
/// `surch_index_postings_codec_*`. Sans eux, un gain disque non réalisé
/// (par exemple un canal `freq` qui ne s'omettrait jamais) passerait
/// inaperçu.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct BlockedEncodeStats {
    /// Nombre de blocs écrits.
    pub blocks: u64,
    /// Octets du canal doc_id, en-tête de bloc et premier identifiant
    /// absolu inclus.
    pub doc_id_bytes: u64,
    /// Octets du canal freq (0 pour un bloc à fréquences toutes égales à 1).
    pub freq_bytes: u64,
    /// Blocs dont le canal freq est totalement ABSENT du payload.
    pub freq_omitted_blocks: u64,
}

impl BlockedEncodeStats {
    /// Accumule `other`. Saturant : un compteur de diagnostic ne doit
    /// jamais pouvoir paniquer une indexation.
    pub fn merge(&mut self, other: Self) {
        self.blocks = self.blocks.saturating_add(other.blocks);
        self.doc_id_bytes = self.doc_id_bytes.saturating_add(other.doc_id_bytes);
        self.freq_bytes = self.freq_bytes.saturating_add(other.freq_bytes);
        self.freq_omitted_blocks = self
            .freq_omitted_blocks
            .saturating_add(other.freq_omitted_blocks);
    }
}

/// Ce que [`encode_postings_blocked_with_directory`] produit en UNE passe :
/// le payload, son répertoire de blocs et les compteurs d'écriture.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockedEncodeOutput {
    pub payload: Vec<u8>,
    pub directory: Vec<BlockDirEntry>,
    pub stats: BlockedEncodeStats,
}

/// Choix du codage du canal doc_id d'un bloc, décidé par coût EXACT.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DocPlan {
    Varint,
    Packed { width: u32 },
    Patched { width: u32, exceptions: usize },
}

/// Décide le codage du canal doc_id d'un bloc à partir de l'histogramme
/// des largeurs de ses deltas.
///
/// La règle est TOTALE et DÉTERMINISTE — c'est ce qui rend l'encodage
/// canonique, donc ce qui permet à `decode_postings_payload_checked`
/// (surch-index) de ré-encoder et de comparer octet pour octet :
/// on parcourt les largeurs par ordre croissant et on ne retient qu'une
/// amélioration STRICTE, en partant du varint. À coût égal, le mode le
/// plus simple gagne donc toujours.
fn plan_doc_channel(histogram: &[usize; 33], delta_count: usize, varint_cost: usize) -> DocPlan {
    if delta_count == 0 {
        return DocPlan::Varint;
    }
    let Some(width_max) = (1..=MAX_PACKED_WIDTH)
        .rev()
        .find(|&w| histogram[w as usize] > 0)
    else {
        // Impossible sur une entrée valide (les deltas intra-bloc sont
        // >= 1 puisque les doc_ids croissent strictement), mais le codec
        // ne doit jamais dépendre d'un invariant qu'il ne vérifie pas.
        return DocPlan::Varint;
    };

    let mut best_plan = DocPlan::Varint;
    let mut best_cost = varint_cost;

    // Largeurs candidates AVEC exceptions, de la plus étroite à la plus
    // large ; puis la largeur maximale, qui est la seule sans exception.
    for width in 1..width_max {
        let mut exceptions = 0usize;
        let mut exception_bytes = 0usize;
        for bits in (width + 1)..=width_max {
            let count = histogram[bits as usize];
            if count == 0 {
                continue;
            }
            exceptions += count;
            // 1 octet de position + le varint de la valeur complète.
            exception_bytes += count * (1 + varint_len_for_bits(bits));
        }
        // `exceptions` tient dans un `u8` : un bloc porte au plus
        // FOR_BLOCK_SIZE - 1 = 127 deltas.
        let cost = 2 + packed_len(delta_count, width) + exception_bytes;
        if cost < best_cost {
            best_cost = cost;
            best_plan = DocPlan::Patched { width, exceptions };
        }
    }

    let packed_cost = 1 + packed_len(delta_count, width_max);
    if packed_cost < best_cost {
        best_plan = DocPlan::Packed { width: width_max };
    }
    best_plan
}

/// Encode UN bloc dans `out`. `scratch` est réutilisé d'un bloc à l'autre
/// pour que l'encodage ne fasse pas une allocation par bloc — le piège
/// « une écriture par token » a déjà coûté -40 % d'indexation sur ce
/// dépôt, on ne le reproduit pas au niveau de l'allocateur.
fn encode_block_into(
    out: &mut Vec<u8>,
    doc_chunk: &[u32],
    freq_chunk: &[u32],
    scratch: &mut Vec<u32>,
    exceptions_scratch: &mut Vec<(u8, u32)>,
) -> BlockedEncodeStats {
    let doc_start = out.len();

    scratch.clear();
    let mut histogram = [0usize; 33];
    let mut varint_cost = 0usize;
    for pair in doc_chunk.windows(2) {
        let delta = pair[1] - pair[0]; // strictement croissant : vérifié en amont
        let bits = bit_width(delta);
        histogram[bits as usize] += 1;
        varint_cost += varint_len_for_bits(bits);
        scratch.push(delta);
    }
    let plan = plan_doc_channel(&histogram, scratch.len(), varint_cost);

    let freq_width = freq_chunk
        .iter()
        .map(|&freq| bit_width(freq))
        .max()
        .unwrap_or(0)
        .max(1);
    let freq_mode = if freq_chunk.iter().all(|&freq| freq == 1) {
        FREQ_MODE_ALL_ONES
    } else if freq_chunk.windows(2).all(|pair| pair[0] == pair[1]) {
        FREQ_MODE_CONSTANT
    } else {
        let freq_varint_cost: usize = freq_chunk
            .iter()
            .map(|&freq| varint_len_for_bits(bit_width(freq)))
            .sum();
        if freq_varint_cost < 1 + packed_len(freq_chunk.len(), freq_width) {
            FREQ_MODE_VARINT
        } else {
            FREQ_MODE_PACKED
        }
    };

    let doc_mode = match plan {
        DocPlan::Varint => DOC_MODE_VARINT,
        DocPlan::Packed { .. } => DOC_MODE_PACKED,
        DocPlan::Patched { .. } => DOC_MODE_PATCHED,
    };
    out.push(doc_mode | (freq_mode << FREQ_MODE_SHIFT));
    // Le premier identifiant reste ABSOLU : c'est ce qui rend un bloc
    // décodable seul, propriété dont dépend le curseur à sauts (un `pread`
    // du seul bloc visé, sans lire ses prédécesseurs).
    write_varint_u32(out, doc_chunk[0]);

    match plan {
        DocPlan::Varint => {
            for &delta in scratch.iter() {
                write_varint_u32(out, delta);
            }
        }
        DocPlan::Packed { width } => {
            out.push(width as u8);
            pack_bits(out, scratch, width);
        }
        DocPlan::Patched { width, exceptions } => {
            out.push(width as u8);
            out.push(exceptions as u8);
            let threshold = 1u32 << width;
            exceptions_scratch.clear();
            for (index, delta) in scratch.iter_mut().enumerate() {
                if *delta >= threshold {
                    exceptions_scratch.push((index as u8, *delta));
                    *delta = 0;
                }
            }
            // Le planificateur compte les exceptions depuis l'histogramme,
            // l'écriture les redécouvre par comparaison au seuil : les deux
            // doivent voir exactement le même ensemble, sinon l'octet de
            // décompte mentirait sur la liste qui le suit.
            debug_assert_eq!(
                exceptions_scratch.len(),
                exceptions,
                "décompte d'exceptions divergent entre le plan et l'écriture"
            );
            pack_bits(out, scratch, width);
            for &(index, delta) in exceptions_scratch.iter() {
                out.push(index);
                write_varint_u32(out, delta);
            }
        }
    }

    let doc_id_bytes = (out.len() - doc_start) as u64;
    let freq_start = out.len();
    match freq_mode {
        FREQ_MODE_CONSTANT => write_varint_u32(out, freq_chunk[0]),
        FREQ_MODE_PACKED => {
            out.push(freq_width as u8);
            pack_bits(out, freq_chunk, freq_width);
        }
        FREQ_MODE_VARINT => {
            for &freq in freq_chunk {
                write_varint_u32(out, freq);
            }
        }
        _ => {}
    }

    BlockedEncodeStats {
        blocks: 1,
        doc_id_bytes,
        freq_bytes: (out.len() - freq_start) as u64,
        freq_omitted_blocks: u64::from(freq_mode == FREQ_MODE_ALL_ONES),
    }
}

/// Encode `(doc_ids, freqs)` en blocs auto-portants d'au plus
/// [`FOR_BLOCK_SIZE`] postings — le format de segment de Lot C
/// `C1a-batché`, révision D2 (bit-packing + omission des fréquences
/// constantes).
///
/// Chaque bloc réinitialise sa base de delta : le premier `doc_id` du bloc
/// est écrit en varint ABSOLU, les suivants en deltas intra-bloc. Un bloc
/// reste donc décodable en isolation par [`decode_block`] à partir de ses
/// seuls octets et de son nombre de postings — c'est la propriété dont
/// dépend le chemin de lecture adressé par `pread` (C1b) : la plage
/// d'octets d'un bloc, lue dans le répertoire, se `pread` et se décode
/// seule.
///
/// Disposition — aucun en-tête global, l'appelant connaît déjà
/// `doc_ids.len()` (la longueur de plage CSR du terme) et en déduit le
/// nombre de blocs par `doc_ids.len().div_ceil(FOR_BLOCK_SIZE)` :
///
/// ```text
/// pour chaque bloc de n <= FOR_BLOCK_SIZE postings :
///     1 octet  en-tête : [doc_mode:2][freq_mode:2][réservé:4 = 0]
///     varint   doc_ids[0] ABSOLU
///     canal doc_id, selon doc_mode :
///       0 varint  : n-1 varints de delta
///       1 packed  : 1 octet de largeur w, puis n-1 deltas sur w bits
///       2 patched : 1 octet w, 1 octet e, n-1 deltas sur w bits (les
///                   exceptions y valent 0), puis e couples
///                   (1 octet de position, varint de la valeur complète)
///     canal freq, selon freq_mode :
///       0 all-ones : ABSENT — toutes les fréquences valent 1
///       1 constant : varint de la constante (!= 1)
///       2 packed   : 1 octet de largeur, puis n fréquences bit-packées
///       3 varint   : n varints (repli, uniquement s'il est plus court)
/// ```
///
/// Le mode d'un bloc est choisi par coût EXACT, jamais par heuristique :
/// le codage retenu n'est donc jamais plus gros que le varint historique,
/// à l'octet d'en-tête près.
///
/// `doc_ids` et `freqs` doivent avoir la même longueur et `doc_ids` doit
/// croître strictement ; une violation rend
/// [`PostingsBlockError::NotMonotonic`] (variante réutilisée plutôt que
/// d'introduire un changement d'énumération cassant, comme sur l'encodeur
/// frère).
pub fn encode_postings_blocked(
    doc_ids: &[u32],
    freqs: &[u32],
) -> Result<Vec<u8>, PostingsBlockError> {
    encode_postings_blocked_with_directory(doc_ids, freqs).map(|output| output.payload)
}

/// Variante de [`encode_postings_blocked`] qui rend AUSSI le répertoire de
/// blocs et les compteurs d'écriture, en UNE seule passe.
///
/// Le chemin d'indexation appelait jusqu'ici `encode_postings_blocked`
/// puis [`block_directory`], qui RE-DÉCODAIT intégralement le payload que
/// l'encodeur venait de produire, uniquement pour retrouver les frontières
/// de blocs et leur `max_doc_id`. L'encodeur connaît les deux : cette
/// fonction supprime ce second passage par terme.
pub fn encode_postings_blocked_with_directory(
    doc_ids: &[u32],
    freqs: &[u32],
) -> Result<BlockedEncodeOutput, PostingsBlockError> {
    if doc_ids.len() != freqs.len() {
        return Err(PostingsBlockError::NotMonotonic);
    }
    validate_strictly_increasing(doc_ids)?;

    // ~1 octet par posting suffit désormais dans le cas nominal (freqs
    // omises, deltas bit-packés) ; on garde une marge de 2 pour ne pas
    // repayer une croissance géométrique sur les listes clairsemées.
    let mut payload = Vec::with_capacity(doc_ids.len() * 2 + 8);
    let mut directory = Vec::with_capacity(doc_ids.len().div_ceil(FOR_BLOCK_SIZE));
    let mut stats = BlockedEncodeStats::default();
    let mut scratch: Vec<u32> = Vec::with_capacity(FOR_BLOCK_SIZE);
    let mut exceptions_scratch: Vec<(u8, u32)> = Vec::new();

    for (doc_chunk, freq_chunk) in doc_ids
        .chunks(FOR_BLOCK_SIZE)
        .zip(freqs.chunks(FOR_BLOCK_SIZE))
    {
        let byte_offset = payload.len();
        stats.merge(encode_block_into(
            &mut payload,
            doc_chunk,
            freq_chunk,
            &mut scratch,
            &mut exceptions_scratch,
        ));
        directory.push(BlockDirEntry {
            byte_offset_in_term_payload: byte_offset as u32,
            count: doc_chunk.len() as u16,
            max_doc_id: *doc_chunk
                .last()
                .expect("chunks() never yields an empty chunk"),
        });
    }

    Ok(BlockedEncodeOutput {
        payload,
        directory,
        stats,
    })
}

/// Cœur de décodage partagé par [`decode_block`] et
/// [`decode_postings_blocked`] : lit UN bloc de `count` postings à partir
/// de `*position`, en avançant `*position` de ce qu'il a consommé.
///
/// Fail-closed : tout octet qu'un encodeur canonique n'aurait pas pu
/// produire (bit réservé posé, mode inconnu, largeur nulle ou > 32,
/// exception qui ne dépasse pas sa largeur, position d'exception hors
/// bloc ou non croissante, constante de fréquence égale à 1) est une
/// corruption explicite, jamais une omission silencieuse.
fn decode_block_at(
    bytes: &[u8],
    position: &mut usize,
    count: usize,
) -> Result<(Vec<u32>, Vec<u32>), PostingsBlockError> {
    if count == 0 {
        return Ok((Vec::new(), Vec::new()));
    }
    let header = *bytes
        .get(*position)
        .ok_or(PostingsBlockError::UnexpectedEof)?;
    *position += 1;
    if header & HEADER_RESERVED_MASK != 0 {
        return Err(PostingsBlockError::MalformedBlock);
    }
    let doc_mode = header & DOC_MODE_MASK;
    let freq_mode = (header >> FREQ_MODE_SHIFT) & DOC_MODE_MASK;

    let first = read_varint_u32(bytes, position)?;
    let delta_count = count - 1;
    let mut deltas: Vec<u32> = Vec::with_capacity(delta_count);
    match doc_mode {
        DOC_MODE_VARINT => {
            for _ in 0..delta_count {
                deltas.push(read_varint_u32(bytes, position)?);
            }
        }
        DOC_MODE_PACKED => {
            let width = read_width(bytes, position)?;
            unpack_bits(bytes, position, delta_count, width, &mut deltas)?;
        }
        DOC_MODE_PATCHED => {
            let width = read_width(bytes, position)?;
            // Une exception vaut « >= 2^width » : la largeur maximale ne
            // laisse aucune valeur au-dessus d'elle, donc l'encodeur ne
            // l'emploie jamais en mode patché. La refuser ici évite en
            // prime un décalage de 32 bits sur `u32`.
            if width >= MAX_PACKED_WIDTH {
                return Err(PostingsBlockError::MalformedBlock);
            }
            let exceptions = usize::from(
                *bytes
                    .get(*position)
                    .ok_or(PostingsBlockError::UnexpectedEof)?,
            );
            *position += 1;
            if exceptions == 0 || exceptions > delta_count {
                return Err(PostingsBlockError::MalformedBlock);
            }
            unpack_bits(bytes, position, delta_count, width, &mut deltas)?;
            let threshold = 1u32 << width;
            let mut previous_index: Option<usize> = None;
            for _ in 0..exceptions {
                let index = usize::from(
                    *bytes
                        .get(*position)
                        .ok_or(PostingsBlockError::UnexpectedEof)?,
                );
                *position += 1;
                if index >= delta_count || previous_index.is_some_and(|prev| index <= prev) {
                    return Err(PostingsBlockError::MalformedBlock);
                }
                let value = read_varint_u32(bytes, position)?;
                if value < threshold || deltas[index] != 0 {
                    return Err(PostingsBlockError::MalformedBlock);
                }
                deltas[index] = value;
                previous_index = Some(index);
            }
        }
        _ => return Err(PostingsBlockError::MalformedBlock),
    }

    let mut doc_ids = Vec::with_capacity(count);
    doc_ids.push(first);
    let mut previous = first;
    for &delta in &deltas {
        if delta == 0 {
            return Err(PostingsBlockError::NotMonotonic);
        }
        let value = previous
            .checked_add(delta)
            .ok_or(PostingsBlockError::NotMonotonic)?;
        doc_ids.push(value);
        previous = value;
    }

    let freqs = match freq_mode {
        FREQ_MODE_ALL_ONES => vec![1u32; count],
        FREQ_MODE_CONSTANT => {
            let constant = read_varint_u32(bytes, position)?;
            if constant == 1 {
                return Err(PostingsBlockError::MalformedBlock);
            }
            vec![constant; count]
        }
        FREQ_MODE_PACKED => {
            let width = read_width(bytes, position)?;
            let mut freqs = Vec::with_capacity(count);
            unpack_bits(bytes, position, count, width, &mut freqs)?;
            freqs
        }
        FREQ_MODE_VARINT => {
            let mut freqs = Vec::with_capacity(count);
            for _ in 0..count {
                freqs.push(read_varint_u32(bytes, position)?);
            }
            freqs
        }
        // Inatteignable : `freq_mode` est masqué sur 2 bits, donc couvert
        // par les quatre bras ci-dessus. Le bras existe parce que le type
        // porté est un `u8`, pas une énumération.
        _ => return Err(PostingsBlockError::MalformedBlock),
    };

    Ok((doc_ids, freqs))
}

/// Lit l'octet de largeur d'un canal bit-packé. `0` et toute valeur au-delà
/// de 32 sont refusées : un encodeur canonique n'en produit jamais, et
/// accepter une largeur nulle rendrait un bloc de deltas tous nuls, donc
/// des identifiants non strictement croissants.
fn read_width(bytes: &[u8], position: &mut usize) -> Result<u32, PostingsBlockError> {
    let width = u32::from(
        *bytes
            .get(*position)
            .ok_or(PostingsBlockError::UnexpectedEof)?,
    );
    *position += 1;
    if width == 0 || width > MAX_PACKED_WIDTH {
        return Err(PostingsBlockError::MalformedBlock);
    }
    Ok(width)
}

/// Decode a single self-contained block produced by
/// [`encode_postings_blocked`]. `count` is the number of postings in the
/// block — known ahead of time by the caller from context (the CSR term
/// length and the block index determine it exactly: [`FOR_BLOCK_SIZE`]
/// for every block but the last, and the remainder — or [`FOR_BLOCK_SIZE`]
/// itself when the remainder is `0` — for the last one; the same
/// arithmetic `doc_ids.chunks(FOR_BLOCK_SIZE)` already performs
/// elsewhere in this codebase). `bytes` must start at the block's first
/// byte; trailing bytes belonging to a following block are simply
/// ignored (decoding stops once `count` doc ids and `count` freqs have
/// been consumed), so callers may pass either the block's exact byte
/// range or the remainder of the term's payload.
pub fn decode_block(
    bytes: &[u8],
    count: usize,
) -> Result<(Vec<u32>, Vec<u32>), PostingsBlockError> {
    let mut position = 0;
    decode_block_at(bytes, &mut position, count)
}

/// Decode the full payload produced by [`encode_postings_blocked`] for a
/// term with `total_count` postings, walking block by block and
/// concatenating the results. Used by the shadow round-trip validation
/// (`PostingsBuilder::build`'s `debug_assert`) and by cold paths that
/// need a term fully materialized rather than block-by-block.
pub fn decode_postings_blocked(
    bytes: &[u8],
    total_count: usize,
) -> Result<(Vec<u32>, Vec<u32>), PostingsBlockError> {
    let mut position = 0;
    let mut doc_ids = Vec::with_capacity(total_count);
    let mut freqs = Vec::with_capacity(total_count);
    let mut remaining = total_count;
    while remaining > 0 {
        let block_count = remaining.min(FOR_BLOCK_SIZE);
        let (block_doc_ids, block_freqs) = decode_block_at(bytes, &mut position, block_count)?;
        doc_ids.extend(block_doc_ids);
        freqs.extend(block_freqs);
        remaining -= block_count;
    }
    Ok((doc_ids, freqs))
}

/// Directory entry describing where one block lives inside a term's
/// [`encode_postings_blocked`] payload — the input `DiskPostingsCursor`
/// (`surch-index`, Lot C C1b sous-pas 1,
/// `docs/paper/c1b-disk-backed-design-2026-07-02.md`) needs to skip whole
/// blocks using ONLY a `max_doc_id` comparison, no `pread`, no decode,
/// before landing on the single block that actually has to be read.
///
/// Deliberately does NOT carry `min_doc_id`: the eventual persisted
/// on-disk directory (design decision 4) only stores `max_doc_id` per
/// block to halve its footprint — a conservative `min` (`max[j-1] + 1`)
/// is enough for a skip decision, and `advance_to` never needs it at all
/// since it only ever compares against `max_doc_id`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BlockDirEntry {
    /// Byte offset of this block's first byte, relative to the START of
    /// the term's own `encode_postings_blocked` payload — NOT
    /// segment-absolute. Callers add the term's own base offset in the
    /// shared segment separately.
    pub byte_offset_in_term_payload: u32,
    /// Number of postings encoded in this block ([`FOR_BLOCK_SIZE`] for
    /// every block but possibly the last).
    pub count: u16,
    /// The last (greatest) doc_id decoded from this block.
    pub max_doc_id: u32,
}

/// Compute the per-block directory of a term's [`encode_postings_blocked`]
/// payload: walks the payload block by block (the same shape as
/// [`decode_postings_blocked`]) and records only `(byte_offset, count,
/// max_doc_id)` per block, discarding each block's decoded doc_ids/freqs
/// once its last doc_id has been read off.
///
/// This is NOT a zero-cost operation — every block's varints are still
/// decoded once to find its `max_doc_id` — it is the ad hoc,
/// not-yet-persisted stand-in for the on-disk directory a later C1b
/// sous-pas will materialise once at build time instead of recomputing
/// it here on every open. Only this function's OUTPUT (the returned
/// `Vec<BlockDirEntry>`) is meant to ship forward; the walk itself is a
/// sous-pas-1 scaffolding cost.
pub fn block_directory(
    bytes: &[u8],
    total_count: usize,
) -> Result<Vec<BlockDirEntry>, PostingsBlockError> {
    let mut position = 0;
    let mut remaining = total_count;
    let mut entries = Vec::with_capacity(total_count.div_ceil(FOR_BLOCK_SIZE));
    while remaining > 0 {
        let block_count = remaining.min(FOR_BLOCK_SIZE);
        let byte_offset = position;
        let (doc_ids, _freqs) = decode_block_at(bytes, &mut position, block_count)?;
        let max_doc_id = *doc_ids
            .last()
            .expect("block_count > 0 (remaining > 0 guard) guarantees a non-empty block");
        entries.push(BlockDirEntry {
            byte_offset_in_term_payload: byte_offset as u32,
            count: block_count as u16,
            max_doc_id,
        });
        remaining -= block_count;
    }
    Ok(entries)
}

/// Inspect the encoded postings payload block-by-block without fully
/// materialising any intermediate posting structs.
pub fn inspect_postings_blocks(bytes: &[u8]) -> Result<Vec<PostingsBlockMeta>, PostingsBlockError> {
    let mut position = 0;
    let length = read_varint_u32(bytes, &mut position)? as usize;
    if length == 0 {
        return Ok(Vec::new());
    }

    let mut blocks = Vec::with_capacity(length.div_ceil(FOR_BLOCK_SIZE));
    let mut previous: u32 = 0;
    let mut started = false;
    let mut remaining = length;
    let mut block_index = 0usize;

    while remaining > 0 {
        let posting_count = remaining.min(FOR_BLOCK_SIZE);
        let doc_ids_byte_start = position;
        let mut min_doc_id = 0u32;
        let mut max_doc_id = 0u32;

        for slot in 0..posting_count {
            let delta = read_varint_u32(bytes, &mut position)?;
            if started && delta == 0 {
                return Err(PostingsBlockError::NotMonotonic);
            }
            let value = previous
                .checked_add(delta)
                .ok_or(PostingsBlockError::NotMonotonic)?;
            if slot == 0 {
                min_doc_id = value;
            }
            max_doc_id = value;
            previous = value;
            started = true;
        }

        blocks.push(PostingsBlockMeta {
            block_index,
            posting_count,
            min_doc_id,
            max_doc_id,
            max_term_freq: 0,
            doc_ids_byte_start,
            doc_ids_byte_end: position,
            freqs_byte_start: 0,
            freqs_byte_end: 0,
        });

        remaining -= posting_count;
        block_index += 1;
    }

    for block in &mut blocks {
        block.freqs_byte_start = position;
        let mut max_term_freq = 0u32;
        for _ in 0..block.posting_count {
            let freq = read_varint_u32(bytes, &mut position)?;
            max_term_freq = max_term_freq.max(freq);
        }
        block.max_term_freq = max_term_freq;
        block.freqs_byte_end = position;
    }

    Ok(blocks)
}

/// Streaming decoder for the doc-id channel of a payload produced by
/// [`encode_postings_doc_id_freq`]. Yields `(doc_id, position_in_bytes)`
/// pairs, **without allocating** a `Vec<u32>`. Used by the future
/// "decode-on-demand" wire-up where the scoring loop walks doc ids one
/// by one and only materialises the matching slice into RAM.
///
/// Phase 1 wire-up plan: callers use `DocIdDeltaCursor::new(encoded)`,
/// then `cursor.next()` returns `Ok(Some(doc_id))` until exhaustion;
/// `Ok(None)` on the natural end, `Err(_)` on a malformed payload.
#[derive(Debug)]
pub struct DocIdDeltaCursor<'a> {
    bytes: &'a [u8],
    position: usize,
    remaining: u32,
    previous: u32,
    /// First delta seen? `0` is only legal for `index == 0`.
    started: bool,
}

impl<'a> DocIdDeltaCursor<'a> {
    /// Build a cursor over `encoded`, reading the length header
    /// eagerly so the caller learns about a truncated payload before
    /// the first `next()` call.
    pub fn new(encoded: &'a [u8]) -> Result<Self, PostingsBlockError> {
        let mut position = 0;
        let remaining = read_varint_u32(encoded, &mut position)?;
        Ok(Self {
            bytes: encoded,
            position,
            remaining,
            previous: 0,
            started: false,
        })
    }

    /// Number of doc ids that have not been yielded yet.
    pub fn remaining(&self) -> u32 {
        self.remaining
    }

    /// Advance one step. Returns `Ok(None)` once the payload is
    /// exhausted, `Err(_)` if the next varint is malformed.
    #[allow(clippy::should_implement_trait)]
    pub fn next(&mut self) -> Result<Option<u32>, PostingsBlockError> {
        if self.remaining == 0 {
            return Ok(None);
        }
        let delta = read_varint_u32(self.bytes, &mut self.position)?;
        if self.started && delta == 0 {
            return Err(PostingsBlockError::NotMonotonic);
        }
        let value = self
            .previous
            .checked_add(delta)
            .ok_or(PostingsBlockError::NotMonotonic)?;
        self.previous = value;
        self.started = true;
        self.remaining -= 1;
        Ok(Some(value))
    }
}

/// Per-block min/max summary used by [`BlockSkipList`]. Kept tiny on
/// purpose: a (`block_index`, `min_doc_id`, `max_doc_id`) triple is
/// what the cursor needs to decide "does target fall in this block or
/// past it?". The full [`PostingsBlockMeta`] is not required because
/// skip decisions never look at the freq channel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BlockSkipEntry {
    pub block_index: usize,
    pub min_doc_id: u32,
    pub max_doc_id: u32,
}
/// Two-level skip list over the per-block min/max doc id range,
/// computed once from the block metadata produced by
/// [`inspect_postings_blocks`] (or from any aligned `BlockMeta` mirror
/// like `surch_index::postings::BlockMeta`). The bottom layer mirrors
/// the per-block doc-id ranges; the top layer keeps one entry per
/// [`SKIP_INTERVAL`] bottom entries to short-circuit the initial
/// binary search on long posting lists.
///
/// The structure stays **derived state** — no new on-disk byte is
/// written. The codec ships an `inspect_postings_blocks` helper that
/// can rebuild the skip list at open time, which keeps the on-disk
/// format backward compatible with everything written before Lot 2.
///
/// **Invariants** preserved by [`BlockSkipList::from_block_ranges`]:
///
/// 1. `bottom` is non-empty, sorted by `block_index`, with strictly
///    increasing `min_doc_id` and `max_doc_id` across consecutive
///    entries (because postings are strictly increasing).
/// 2. For each bottom entry `b`, `b.min_doc_id <= b.max_doc_id`.
/// 3. `top[i] = bottom[i * SKIP_INTERVAL]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockSkipList {
    /// One entry per FoR block, in `block_index` order.
    bottom: Vec<BlockSkipEntry>,
    /// One entry per [`SKIP_INTERVAL`] bottom entries. Empty iff
    /// `bottom` has 0 or 1 entries (no skip benefit).
    top: Vec<BlockSkipEntry>,
}

impl BlockSkipList {
    /// Build a skip list from an iterator of `(block_index, min_doc_id,
    /// max_doc_id)` triples produced in `block_index` order. Returns
    /// [`PostingsBlockError::NotMonotonic`] if any invariant is broken
    /// (out-of-order indices, decreasing min/max, or `min > max`).
    pub fn from_block_ranges<I>(blocks: I) -> Result<Self, PostingsBlockError>
    where
        I: IntoIterator<Item = BlockSkipEntry>,
    {
        let bottom: Vec<BlockSkipEntry> = blocks.into_iter().collect();
        for window in bottom.windows(2) {
            let prev = &window[0];
            let next = &window[1];
            if prev.block_index >= next.block_index {
                return Err(PostingsBlockError::NotMonotonic);
            }
            if prev.max_doc_id >= next.min_doc_id {
                return Err(PostingsBlockError::NotMonotonic);
            }
        }
        for entry in &bottom {
            if entry.min_doc_id > entry.max_doc_id {
                return Err(PostingsBlockError::NotMonotonic);
            }
        }
        let top = if bottom.len() <= 1 {
            Vec::new()
        } else {
            bottom.iter().step_by(SKIP_INTERVAL).copied().collect()
        };
        Ok(Self { bottom, top })
    }

    /// Convenience constructor from a slice of [`PostingsBlockMeta`].
    pub fn from_block_metas(metas: &[PostingsBlockMeta]) -> Result<Self, PostingsBlockError> {
        Self::from_block_ranges(metas.iter().map(|meta| BlockSkipEntry {
            block_index: meta.block_index,
            min_doc_id: meta.min_doc_id,
            max_doc_id: meta.max_doc_id,
        }))
    }

    /// Returns the bottom layer (one [`BlockSkipEntry`] per FoR block).
    pub fn entries(&self) -> &[BlockSkipEntry] {
        &self.bottom
    }

    /// Returns the top layer (one entry per [`SKIP_INTERVAL`] blocks).
    /// Empty when the bottom layer has 0 or 1 entries.
    pub fn top_layer(&self) -> &[BlockSkipEntry] {
        &self.top
    }

    /// Build a cursor over this skip list starting at block 0.
    pub fn cursor(&self) -> BlockSkipCursor<'_> {
        BlockSkipCursor {
            skip_list: self,
            position: 0,
            blocks_visited: 0,
        }
    }
}

/// Cursor over a [`BlockSkipList`] supporting `advance_to(target)` with
/// O(log N_blocks) worst case via a two-level skip. Tracks the number
/// of bottom-layer block entries it touched so tests (and an eventual
/// perf counter) can assert that skipping actually skipped blocks
/// rather than walking them one by one.
///
/// The cursor is **monotonic**: each `advance_to(target)` must use a
/// `target` >= every previous one (Lucene `DocIdSetIterator`
/// semantics). Returning to a smaller target would silently produce
/// wrong results upstream, so the cursor never rewinds — the
/// `position` cursor moves forward only.
#[derive(Debug)]
pub struct BlockSkipCursor<'a> {
    skip_list: &'a BlockSkipList,
    /// Index into `skip_list.bottom` of the next candidate block (the
    /// block we will inspect next, or one past the end if exhausted).
    position: usize,
    /// Number of bottom-layer entries the cursor has touched (read /
    /// compared / returned). Used by tests and future
    /// observability to assert that skipping actually skipped blocks.
    blocks_visited: usize,
}

impl<'a> BlockSkipCursor<'a> {
    /// Build a cursor positioned at `position` (typically returned by a
    /// prior `.position()` call). The cursor remains monotonic — calling
    /// `advance_to(target)` with a `target` smaller than the entry at
    /// `position` is allowed but the cursor will still only move
    /// forward. The visit counter restarts at zero, so callers that
    /// want a cumulative count must accumulate themselves.
    pub fn resume(skip_list: &'a BlockSkipList, position: usize) -> Self {
        Self {
            skip_list,
            position,
            blocks_visited: 0,
        }
    }

    /// Returns the current bottom-layer position. `entries()[position()]`
    /// is the candidate that the next `advance_to` will consider first.
    pub fn position(&self) -> usize {
        self.position
    }

    /// Returns the number of bottom-layer entries the cursor has
    /// touched so far. Read by tests to assert skip-list effectiveness.
    pub fn blocks_visited(&self) -> usize {
        self.blocks_visited
    }

    /// Returns the next [`BlockSkipEntry`] whose `max_doc_id >= target`
    /// without advancing past entries that may still contain target.
    ///
    /// - Returns `Some(entry)` and advances `position` to it. Caller
    ///   then decides whether to consume the block (if
    ///   `entry.min_doc_id <= target <= entry.max_doc_id`) or skip
    ///   past it (if `target < entry.min_doc_id`, meaning target falls
    ///   in the gap between two blocks).
    /// - Returns `None` once every block has been exhausted (target
    ///   is past the last `max_doc_id`).
    pub fn advance_to(&mut self, target: u32) -> Option<BlockSkipEntry> {
        let bottom = &self.skip_list.bottom;
        if self.position >= bottom.len() {
            return None;
        }

        // Top-layer jump: walk the top entries forward in steps of
        // SKIP_INTERVAL bottom blocks until the next top entry would
        // overshoot the target. This bumps `position` in O(N_top)
        // worst case while only touching `top` slots (constant
        // per slot), then the bottom-layer walk below does the final
        // O(SKIP_INTERVAL) refinement.
        let top = &self.skip_list.top;
        if !top.is_empty() {
            let mut top_index = self.position / SKIP_INTERVAL;
            // Move forward as long as the *next* top entry's
            // max_doc_id is still < target (i.e. the current top
            // chunk's last block is past). Equivalently: while
            // top[top_index + 1].max_doc_id < target, jump.
            while top_index + 1 < top.len() && top[top_index + 1].max_doc_id < target {
                top_index += 1;
            }
            let candidate_position = top_index * SKIP_INTERVAL;
            if candidate_position > self.position {
                self.position = candidate_position;
            }
        }

        // Bottom-layer linear scan from `position`. The cap (`stop`)
        // is `min(position + SKIP_INTERVAL + 1, bottom.len())`: after
        // the top-layer jump we know the answer is in the current
        // top chunk (or its successor by one slot in the edge case
        // where target lands on a top boundary).
        let stop = bottom
            .len()
            .min(self.position.saturating_add(SKIP_INTERVAL * 2));
        while self.position < stop {
            let entry = bottom[self.position];
            self.blocks_visited += 1;
            if entry.max_doc_id >= target {
                return Some(entry);
            }
            self.position += 1;
        }

        // Edge case: if we exited the bounded scan without finding the
        // target (e.g. tiny posting lists where the top layer was
        // empty and the linear scan didn't quite reach), fall back to
        // a full linear scan to honour the API contract. In practice
        // this branch only fires on degenerate inputs, but it keeps
        // the cursor correct.
        while self.position < bottom.len() {
            let entry = bottom[self.position];
            self.blocks_visited += 1;
            if entry.max_doc_id >= target {
                return Some(entry);
            }
            self.position += 1;
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_empty() {
        let input: [u32; 0] = [];
        let encoded = encode_doc_ids_delta_varint(&input).unwrap();
        // Just the length varint = 0.
        assert_eq!(encoded, vec![0x00]);
        assert_eq!(
            decode_doc_ids_delta_varint(&encoded).unwrap(),
            input.to_vec()
        );
    }

    #[test]
    fn round_trip_single_zero() {
        let input = [0_u32];
        assert_eq!(round_trip(&input), input.to_vec());
    }

    #[test]
    fn round_trip_sequential_small() {
        let input: Vec<u32> = (1..=100).collect();
        assert_eq!(round_trip(&input), input);
    }

    #[test]
    fn round_trip_gapped() {
        let input = [0_u32, 128, 256, 384];
        assert_eq!(round_trip(&input), input.to_vec());
    }

    #[test]
    fn round_trip_extreme_values() {
        let input = [0_u32, u32::MAX / 2, u32::MAX];
        assert_eq!(round_trip(&input), input.to_vec());
    }

    #[test]
    fn round_trip_random_2k_seeded() {
        // Deterministic xorshift32 with a fixed seed so the test is
        // reproducible without pulling in an `rand` dependency.
        let mut state: u32 = 0x1234_5678;
        let mut next = || -> u32 {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            state
        };

        let mut ids: Vec<u32> = (0..2000).map(|_| next() % 10_000_000).collect();
        ids.sort_unstable();
        ids.dedup();

        let encoded = encode_doc_ids_delta_varint(&ids).unwrap();
        let decoded = decode_doc_ids_delta_varint(&encoded).unwrap();
        assert_eq!(decoded, ids);
    }

    #[test]
    fn encode_rejects_non_monotonic() {
        let err = encode_doc_ids_delta_varint(&[1, 0]).unwrap_err();
        assert_eq!(err, PostingsBlockError::NotMonotonic);
    }

    #[test]
    fn encode_rejects_equal_neighbors() {
        let err = encode_doc_ids_delta_varint(&[3, 3]).unwrap_err();
        assert_eq!(err, PostingsBlockError::NotMonotonic);
    }

    #[test]
    fn decode_rejects_truncated_input() {
        // Announce 3 deltas but only provide 1 (plus the length byte).
        let bytes = [0x03_u8, 0x01];
        let err = decode_doc_ids_delta_varint(&bytes).unwrap_err();
        assert_eq!(err, PostingsBlockError::UnexpectedEof);
    }

    #[test]
    fn decode_rejects_empty_input() {
        // Even the length varint is missing.
        let bytes: [u8; 0] = [];
        let err = decode_doc_ids_delta_varint(&bytes).unwrap_err();
        assert_eq!(err, PostingsBlockError::UnexpectedEof);
    }

    #[test]
    fn decode_rejects_varint_overflow() {
        // Length=1, then a 6-byte varint where every byte has the continuation
        // bit set: this overflows u32 well before terminating.
        let bytes = [0x01_u8, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff];
        let err = decode_doc_ids_delta_varint(&bytes).unwrap_err();
        assert_eq!(err, PostingsBlockError::VarintOverflow);
    }

    #[test]
    fn decode_rejects_non_monotonic_zero_delta() {
        // Length=2, first delta=1 (value=1), second delta=0 → not strictly
        // increasing.
        let bytes = [0x02_u8, 0x01, 0x00];
        let err = decode_doc_ids_delta_varint(&bytes).unwrap_err();
        assert_eq!(err, PostingsBlockError::NotMonotonic);
    }

    #[test]
    fn postings_round_trip_empty() {
        let doc_ids: [u32; 0] = [];
        let freqs: [u32; 0] = [];
        let encoded = encode_postings_doc_id_freq(&doc_ids, &freqs).unwrap();
        let (out_ids, out_freqs) = decode_postings_doc_id_freq(&encoded).unwrap();
        assert!(out_ids.is_empty() && out_freqs.is_empty());
    }

    #[test]
    fn postings_round_trip_small() {
        let doc_ids = [1_u32, 3, 7, 9, 42];
        let freqs = [1_u32, 5, 2, 1, 17];
        let encoded = encode_postings_doc_id_freq(&doc_ids, &freqs).unwrap();
        let (out_ids, out_freqs) = decode_postings_doc_id_freq(&encoded).unwrap();
        assert_eq!(out_ids, doc_ids);
        assert_eq!(out_freqs, freqs);
    }

    #[test]
    fn postings_round_trip_2k_seeded_matches_ground_truth() {
        // Same xorshift32 seed as `round_trip_random_2k_seeded` so we
        // share the doc-id corpus with the doc-id-only path.
        let mut state: u32 = 0x1234_5678;
        let mut next = || -> u32 {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            state
        };

        let mut doc_ids: Vec<u32> = (0..2000).map(|_| next() % 10_000_000).collect();
        doc_ids.sort_unstable();
        doc_ids.dedup();
        // Synthetic freqs in [1..=16] — covers the BAN/INSEE token range.
        let freqs: Vec<u32> = doc_ids.iter().map(|d| (d % 16) + 1).collect();

        let encoded = encode_postings_doc_id_freq(&doc_ids, &freqs).unwrap();
        let (out_ids, out_freqs) = decode_postings_doc_id_freq(&encoded).unwrap();
        assert_eq!(out_ids, doc_ids, "decoded doc_ids must equal ground truth");
        assert_eq!(out_freqs, freqs, "decoded freqs must equal ground truth");
    }

    #[test]
    fn postings_rejects_length_mismatch() {
        let err = encode_postings_doc_id_freq(&[1, 2], &[1]).unwrap_err();
        // Reused variant — see doc comment on `encode_postings_doc_id_freq`.
        assert_eq!(err, PostingsBlockError::NotMonotonic);
    }

    #[test]
    fn cursor_yields_same_sequence_as_decode() {
        let doc_ids = [1_u32, 3, 7, 9, 42, 100, 128, 5000];
        let freqs = [1_u32; 8];
        let encoded = encode_postings_doc_id_freq(&doc_ids, &freqs).unwrap();

        let mut cursor = DocIdDeltaCursor::new(&encoded).unwrap();
        assert_eq!(cursor.remaining(), doc_ids.len() as u32);
        let mut collected = Vec::with_capacity(doc_ids.len());
        while let Some(v) = cursor.next().unwrap() {
            collected.push(v);
        }
        assert_eq!(collected, doc_ids);
        // Subsequent calls keep returning None without erroring.
        assert!(cursor.next().unwrap().is_none());
        assert_eq!(cursor.remaining(), 0);
    }

    #[test]
    fn cursor_rejects_truncated_payload() {
        // Announce 3 deltas but only deliver 1.
        let bytes = [0x03_u8, 0x01];
        let mut cursor = DocIdDeltaCursor::new(&bytes).unwrap();
        let _first = cursor.next().unwrap();
        // Second delta read should fail — EOF before continuation.
        let err = cursor.next().unwrap_err();
        assert_eq!(err, PostingsBlockError::UnexpectedEof);
    }

    /// Sanity: the codec payload is meaningfully smaller than the
    /// naive `Vec<u32>` layout for a realistic dense posting list.
    /// This isn't a perf bench (see `benches/for_decode.rs`) — just a
    /// guardrail so regressions in `encode_postings_doc_id_freq` fail
    /// loudly.
    #[test]
    fn postings_compression_ratio_better_than_naive() {
        let doc_ids: Vec<u32> = (0..1024).map(|i| i * 3 + 7).collect();
        let freqs: Vec<u32> = vec![1; 1024];
        let encoded = encode_postings_doc_id_freq(&doc_ids, &freqs).unwrap();
        let naive_bytes = doc_ids.len() * 4 + freqs.len() * 4; // raw u32 + u32
        assert!(
            encoded.len() < naive_bytes / 2,
            "codec={} naive={} — expected at least 2× compression",
            encoded.len(),
            naive_bytes
        );
    }

    #[test]
    fn inspect_postings_blocks_splits_payload_on_block_128_boundary() {
        let doc_ids: Vec<u32> = (0..129).map(|i| i * 3 + 7).collect();
        let freqs: Vec<u32> = (0..129).map(|i| (i % 17) as u32 + 1).collect();
        let encoded = encode_postings_doc_id_freq(&doc_ids, &freqs).unwrap();

        let blocks = inspect_postings_blocks(&encoded).unwrap();
        assert_eq!(blocks.len(), 2);

        assert_eq!(blocks[0].block_index, 0);
        assert_eq!(blocks[0].posting_count, 128);
        assert_eq!(blocks[0].min_doc_id, doc_ids[0]);
        assert_eq!(blocks[0].max_doc_id, doc_ids[127]);
        assert_eq!(blocks[0].max_term_freq, 17);

        assert_eq!(blocks[1].block_index, 1);
        assert_eq!(blocks[1].posting_count, 1);
        assert_eq!(blocks[1].min_doc_id, doc_ids[128]);
        assert_eq!(blocks[1].max_doc_id, doc_ids[128]);
        assert_eq!(blocks[1].max_term_freq, freqs[128]);

        assert!(blocks[0].doc_ids_byte_end <= blocks[1].doc_ids_byte_start);
        assert!(blocks[0].freqs_byte_end <= blocks[1].freqs_byte_start);
        assert_eq!(blocks[1].freqs_byte_end, encoded.len());
    }

    #[test]
    fn inspect_postings_blocks_matches_decoded_chunks_on_seeded_corpus() {
        let mut state: u32 = 0x1234_5678;
        let mut next = || -> u32 {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            state
        };

        let mut doc_ids: Vec<u32> = (0..2000).map(|_| next() % 10_000_000).collect();
        doc_ids.sort_unstable();
        doc_ids.dedup();
        let freqs: Vec<u32> = doc_ids.iter().map(|d| (d % 16) + 1).collect();
        let encoded = encode_postings_doc_id_freq(&doc_ids, &freqs).unwrap();

        let blocks = inspect_postings_blocks(&encoded).unwrap();
        assert_eq!(blocks.len(), doc_ids.len().div_ceil(FOR_BLOCK_SIZE));
        assert_eq!(
            blocks.iter().map(|meta| meta.posting_count).sum::<usize>(),
            doc_ids.len()
        );

        for (meta, (doc_chunk, freq_chunk)) in blocks.iter().zip(
            doc_ids
                .chunks(FOR_BLOCK_SIZE)
                .zip(freqs.chunks(FOR_BLOCK_SIZE)),
        ) {
            assert_eq!(meta.posting_count, doc_chunk.len());
            assert_eq!(meta.min_doc_id, *doc_chunk.first().unwrap());
            assert_eq!(meta.max_doc_id, *doc_chunk.last().unwrap());
            assert_eq!(meta.max_term_freq, *freq_chunk.iter().max().unwrap());
        }
    }

    #[test]
    fn inspect_postings_blocks_rejects_truncated_freq_tail() {
        let doc_ids: Vec<u32> = (0..130).collect();
        let freqs: Vec<u32> = vec![1; 130];
        let mut encoded = encode_postings_doc_id_freq(&doc_ids, &freqs).unwrap();
        encoded.pop();

        let err = inspect_postings_blocks(&encoded).unwrap_err();
        assert_eq!(err, PostingsBlockError::UnexpectedEof);
    }

    #[test]
    fn blocked_roundtrip() {
        // Multi-block corpus: FOR_BLOCK_SIZE*3 + 17 postings (three full
        // blocks + one partial trailing block), irregular gaps so the
        // block-local delta reset is actually exercised (not just a
        // constant stride). `encode_postings_blocked` + `decode_postings_blocked`
        // must round-trip bit-identically to the RAM source, AND
        // `decode_block` on each individual block byte range must match
        // the corresponding chunk decoded via `decode_postings_blocked` —
        // i.e. blocks are genuinely self-contained, not just an artifact
        // of decoding sequentially from offset 0.
        let total: u32 = (FOR_BLOCK_SIZE * 3 + 17) as u32;
        // `i*7 + i%5`: strictly increasing (min step 7 + (%5 delta) = 3),
        // irregular gaps (3 or 8) so the block-local delta reset is exercised.
        // NOT `i*3 + i%7` — that DECREASES at i≡6 mod 7 (step 3-6 = -3), which
        // `encode_postings_blocked` rightly rejects as non-monotonic.
        let doc_ids: Vec<u32> = (0..total).map(|i| i * 7 + (i % 5)).collect();
        let freqs: Vec<u32> = (0..total).map(|i| (i % 16) + 1).collect();

        let encoded = encode_postings_blocked(&doc_ids, &freqs).unwrap();
        let (decoded_doc_ids, decoded_freqs) =
            decode_postings_blocked(&encoded, doc_ids.len()).unwrap();
        assert_eq!(decoded_doc_ids, doc_ids);
        assert_eq!(decoded_freqs, freqs);

        // Per-block random access: re-encode each chunk on its own via
        // `encode_postings_blocked` (single block in, single block out —
        // the delta base resets to 0 either way) and check `decode_block`
        // on that standalone payload matches the corresponding chunk of
        // the full decode. This is the "decodable without predecessors"
        // property C1b's pread path depends on.
        for (doc_chunk, freq_chunk) in doc_ids
            .chunks(FOR_BLOCK_SIZE)
            .zip(freqs.chunks(FOR_BLOCK_SIZE))
        {
            let block_bytes = encode_postings_blocked(doc_chunk, freq_chunk).unwrap();
            let (block_doc_ids, block_freqs) = decode_block(&block_bytes, doc_chunk.len()).unwrap();
            assert_eq!(block_doc_ids, doc_chunk);
            assert_eq!(block_freqs, freq_chunk);
        }
    }

    #[test]
    fn blocked_roundtrip_empty() {
        let doc_ids: [u32; 0] = [];
        let freqs: [u32; 0] = [];
        let encoded = encode_postings_blocked(&doc_ids, &freqs).unwrap();
        assert!(encoded.is_empty());
        let (decoded_doc_ids, decoded_freqs) = decode_postings_blocked(&encoded, 0).unwrap();
        assert!(decoded_doc_ids.is_empty() && decoded_freqs.is_empty());
    }

    #[test]
    fn blocked_roundtrip_exact_block_boundary() {
        // Exactly FOR_BLOCK_SIZE postings: a single full block, no
        // trailing partial block.
        let doc_ids: Vec<u32> = (0..FOR_BLOCK_SIZE as u32).map(|i| i * 2).collect();
        let freqs: Vec<u32> = vec![1; FOR_BLOCK_SIZE];
        let encoded = encode_postings_blocked(&doc_ids, &freqs).unwrap();
        let (decoded_doc_ids, decoded_freqs) =
            decode_postings_blocked(&encoded, doc_ids.len()).unwrap();
        assert_eq!(decoded_doc_ids, doc_ids);
        assert_eq!(decoded_freqs, freqs);
    }

    #[test]
    fn block_directory_matches_chunk_boundaries_and_max_doc_id() {
        // Same multi-block corpus as `blocked_roundtrip` (three full
        // blocks + one partial trailing block, irregular gaps). The
        // directory must report one entry per `chunks(FOR_BLOCK_SIZE)`
        // chunk, with `count`/`max_doc_id` matching that chunk exactly
        // and `byte_offset_in_term_payload` pointing at genuinely
        // independent, growing byte ranges (each block's own
        // `encode_postings_blocked` output, re-decoded standalone via
        // `decode_block`, must match the chunk too — the same
        // "decodable in isolation" property `blocked_roundtrip` checks
        // for `decode_block`, now checked for the directory's offsets).
        let total: u32 = (FOR_BLOCK_SIZE * 3 + 17) as u32;
        let doc_ids: Vec<u32> = (0..total).map(|i| i * 7 + (i % 5)).collect();
        let freqs: Vec<u32> = (0..total).map(|i| (i % 16) + 1).collect();
        let encoded = encode_postings_blocked(&doc_ids, &freqs).unwrap();

        let directory = block_directory(&encoded, doc_ids.len()).unwrap();
        assert_eq!(
            directory.len(),
            4,
            "3 full blocks + 1 partial trailing block"
        );

        let mut previous_end: Option<u32> = None;
        for (dir_entry, (doc_chunk, freq_chunk)) in directory.iter().zip(
            doc_ids
                .chunks(FOR_BLOCK_SIZE)
                .zip(freqs.chunks(FOR_BLOCK_SIZE)),
        ) {
            assert_eq!(dir_entry.count as usize, doc_chunk.len());
            assert_eq!(dir_entry.max_doc_id, *doc_chunk.last().unwrap());
            // Byte ranges strictly grow block over block (no overlap).
            if let Some(prev) = previous_end {
                assert!(dir_entry.byte_offset_in_term_payload >= prev);
            }
            previous_end = Some(dir_entry.byte_offset_in_term_payload);

            // The directory's offset genuinely lands on this block's own
            // bytes: decoding from that offset (to the end of the
            // payload, per `decode_block`'s "remainder of the term's
            // payload" contract) reproduces the chunk exactly.
            let from_offset = &encoded[dir_entry.byte_offset_in_term_payload as usize..];
            let (block_doc_ids, block_freqs) =
                decode_block(from_offset, dir_entry.count as usize).unwrap();
            assert_eq!(block_doc_ids, doc_chunk);
            assert_eq!(block_freqs, freq_chunk);
        }
    }

    #[test]
    fn block_directory_rejects_truncated_payload() {
        let doc_ids: Vec<u32> = (0..130).collect();
        let freqs: Vec<u32> = vec![1; 130];
        let mut encoded = encode_postings_blocked(&doc_ids, &freqs).unwrap();
        encoded.pop();

        let err = block_directory(&encoded, doc_ids.len()).unwrap_err();
        assert_eq!(err, PostingsBlockError::UnexpectedEof);
    }

    #[test]
    fn blocked_rejects_length_mismatch() {
        let err = encode_postings_blocked(&[1, 2], &[1]).unwrap_err();
        assert_eq!(err, PostingsBlockError::NotMonotonic);
    }

    #[test]
    fn blocked_rejects_non_monotonic() {
        let err = encode_postings_blocked(&[5, 4], &[1, 1]).unwrap_err();
        assert_eq!(err, PostingsBlockError::NotMonotonic);
    }

    #[test]
    fn blocked_first_doc_id_may_be_zero_only_at_block_start() {
        // Doc id 0 is legal as the very first element (delta 0 from the
        // block-local base), but nowhere else — same rule as
        // `encode_doc_ids_delta_varint`, just re-applied per block here.
        let doc_ids = [0_u32, 1, 2];
        let freqs = [1_u32, 1, 1];
        let encoded = encode_postings_blocked(&doc_ids, &freqs).unwrap();
        let (decoded_doc_ids, decoded_freqs) =
            decode_postings_blocked(&encoded, doc_ids.len()).unwrap();
        assert_eq!(decoded_doc_ids, doc_ids);
        assert_eq!(decoded_freqs, freqs);
    }

    // ---------------------------------------------------------------
    // D2 — bit-packing des deltas et omission des fréquences constantes.
    // ---------------------------------------------------------------

    /// Taille qu'aurait le payload dans le format ANTÉRIEUR à D2 (un
    /// varint par delta, un varint par fréquence, aucun en-tête de bloc).
    /// Sert de témoin aux tests de non-régression de taille.
    fn legacy_blocked_len(doc_ids: &[u32], freqs: &[u32]) -> usize {
        let mut total = 0usize;
        for (doc_chunk, freq_chunk) in doc_ids
            .chunks(FOR_BLOCK_SIZE)
            .zip(freqs.chunks(FOR_BLOCK_SIZE))
        {
            let mut scratch = Vec::new();
            let mut previous = 0u32;
            for &value in doc_chunk {
                write_varint_u32(&mut scratch, value - previous);
                previous = value;
            }
            for &freq in freq_chunk {
                write_varint_u32(&mut scratch, freq);
            }
            total += scratch.len();
        }
        total
    }

    /// Aller-retour STRICT : les identifiants ET les fréquences relus sont
    /// identiques, dans le même ordre, par le décodeur complet ET par le
    /// décodage bloc par bloc (la propriété dont dépend le curseur disque).
    fn assert_strict_round_trip(doc_ids: &[u32], freqs: &[u32]) -> Vec<u8> {
        let output = encode_postings_blocked_with_directory(doc_ids, freqs).unwrap();
        let (decoded_ids, decoded_freqs) =
            decode_postings_blocked(&output.payload, doc_ids.len()).unwrap();
        assert_eq!(decoded_ids, doc_ids, "doc_ids doivent être identiques");
        assert_eq!(decoded_freqs, freqs, "freqs doivent être identiques");

        // Canonicité : c'est elle que `decode_postings_payload_checked`
        // (surch-index) utilise comme contrôle d'intégrité.
        let reencoded = encode_postings_blocked(&decoded_ids, &decoded_freqs).unwrap();
        assert_eq!(reencoded, output.payload, "l'encodage doit être canonique");

        // Le répertoire rendu par l'encodeur doit être EXACTEMENT celui que
        // le recalcul depuis le payload produit — le chemin de repli
        // `DiskPostingsCursor::open` en dépend.
        let recomputed = block_directory(&output.payload, doc_ids.len()).unwrap();
        assert_eq!(recomputed, output.directory, "répertoires divergents");

        // Chaque bloc est décodable SEUL depuis son offset.
        for (entry, (doc_chunk, freq_chunk)) in output.directory.iter().zip(
            doc_ids
                .chunks(FOR_BLOCK_SIZE)
                .zip(freqs.chunks(FOR_BLOCK_SIZE)),
        ) {
            assert_eq!(entry.count as usize, doc_chunk.len());
            assert_eq!(entry.max_doc_id, *doc_chunk.last().unwrap());
            let from_offset = &output.payload[entry.byte_offset_in_term_payload as usize..];
            let (block_ids, block_freqs) = decode_block(from_offset, doc_chunk.len()).unwrap();
            assert_eq!(block_ids, doc_chunk);
            assert_eq!(block_freqs, freq_chunk);
        }
        output.payload
    }

    #[test]
    fn d2_freqs_toutes_a_un_disparaissent_du_payload() {
        // Le cas NOMINAL du corpus deces : `tf = 1` partout. Le canal des
        // fréquences doit être totalement absent des octets écrits.
        let doc_ids: Vec<u32> = (0..1000).map(|i| i * 3 + 7).collect();
        let freqs: Vec<u32> = vec![1; 1000];
        let output = encode_postings_blocked_with_directory(&doc_ids, &freqs).unwrap();
        assert_eq!(output.stats.freq_bytes, 0, "aucun octet de fréquence");
        assert_eq!(output.stats.freq_omitted_blocks, output.stats.blocks);
        assert_eq!(
            output.stats.blocks,
            doc_ids.len().div_ceil(FOR_BLOCK_SIZE) as u64
        );
        assert_eq!(
            output.stats.doc_id_bytes as usize,
            output.payload.len(),
            "tout le payload est du canal doc_id"
        );
        assert_strict_round_trip(&doc_ids, &freqs);
    }

    #[test]
    fn d2_freq_constante_non_unitaire_tient_en_un_varint_par_bloc() {
        let doc_ids: Vec<u32> = (0..300).map(|i| i * 5).collect();
        let freqs: Vec<u32> = vec![9; 300];
        let output = encode_postings_blocked_with_directory(&doc_ids, &freqs).unwrap();
        // 3 blocs (128 + 128 + 44), un varint de la constante chacun.
        assert_eq!(output.stats.blocks, 3);
        assert_eq!(output.stats.freq_bytes, 3);
        assert_eq!(output.stats.freq_omitted_blocks, 0);
        assert_strict_round_trip(&doc_ids, &freqs);
    }

    #[test]
    fn d2_freqs_variables_restent_exactes() {
        let doc_ids: Vec<u32> = (0..500).map(|i| i * 2 + 1).collect();
        let freqs: Vec<u32> = doc_ids.iter().map(|d| (d % 37) + 1).collect();
        let output = encode_postings_blocked_with_directory(&doc_ids, &freqs).unwrap();
        assert!(output.stats.freq_bytes > 0, "canal freq réellement écrit");
        assert_eq!(output.stats.freq_omitted_blocks, 0);
        assert_strict_round_trip(&doc_ids, &freqs);
    }

    #[test]
    fn d2_freqs_mixtes_par_bloc() {
        // Un bloc à fréquences toutes égales à 1, un bloc constant != 1, un
        // bloc variable : les trois modes coexistent dans un même terme.
        let total = FOR_BLOCK_SIZE * 3;
        let doc_ids: Vec<u32> = (0..total as u32).map(|i| i * 4).collect();
        let freqs: Vec<u32> = (0..total)
            .map(|i| {
                if i < FOR_BLOCK_SIZE {
                    1
                } else if i < FOR_BLOCK_SIZE * 2 {
                    4
                } else {
                    (i % 11) as u32 + 1
                }
            })
            .collect();
        let output = encode_postings_blocked_with_directory(&doc_ids, &freqs).unwrap();
        assert_eq!(output.stats.blocks, 3);
        assert_eq!(output.stats.freq_omitted_blocks, 1);
        assert_strict_round_trip(&doc_ids, &freqs);
    }

    #[test]
    fn d2_df_de_un_et_df_de_deux() {
        assert_strict_round_trip(&[0], &[1]);
        assert_strict_round_trip(&[u32::MAX], &[1]);
        assert_strict_round_trip(&[7], &[42]);
        assert_strict_round_trip(&[0, u32::MAX], &[1, 1]);
        assert_strict_round_trip(&[3, 4], &[2, 3]);
    }

    #[test]
    fn d2_bloc_partiel_en_fin_de_terme() {
        // Trois blocs pleins + un bloc partiel de 17 postings — la forme
        // qu'un terme de queue de segment produit toujours.
        let total = (FOR_BLOCK_SIZE * 3 + 17) as u32;
        let doc_ids: Vec<u32> = (0..total).map(|i| i * 7 + (i % 5)).collect();
        let freqs: Vec<u32> = (0..total).map(|i| (i % 16) + 1).collect();
        let output = encode_postings_blocked_with_directory(&doc_ids, &freqs).unwrap();
        assert_eq!(output.directory.len(), 4);
        assert_eq!(output.directory[3].count, 17);
        assert_strict_round_trip(&doc_ids, &freqs);
    }

    #[test]
    fn d2_df_tres_grand_et_dense() {
        // 100 000 postings très denses : le cas où le bit-packing gagne
        // franchement contre le varint (deltas de 1 à 3 bits).
        let doc_ids: Vec<u32> = (0..100_000).map(|i| i * 2).collect();
        let freqs: Vec<u32> = vec![1; 100_000];
        let payload = assert_strict_round_trip(&doc_ids, &freqs);
        let legacy = legacy_blocked_len(&doc_ids, &freqs);
        assert!(
            payload.len() * 4 < legacy,
            "attendu au moins 4x plus petit : {} contre {legacy}",
            payload.len()
        );
    }

    #[test]
    fn d2_valeurs_aberrantes_ne_font_pas_exploser_le_bloc() {
        // Le motif RÉEL du corpus deces sur un champ groupé (code INSEE de
        // décès) : des runs très denses séparés par un saut énorme. Un
        // frame-of-reference NAÏF prendrait la largeur du saut pour TOUTES
        // les valeurs du bloc ; le mode à exceptions doit l'éviter.
        let mut doc_ids: Vec<u32> = Vec::new();
        let mut next = 0u32;
        for run in 0..8u32 {
            for _ in 0..100 {
                doc_ids.push(next);
                next += 1;
            }
            next += 900_000 + run; // frontière de grappe
        }
        let freqs: Vec<u32> = vec![1; doc_ids.len()];
        let payload = assert_strict_round_trip(&doc_ids, &freqs);
        let legacy = legacy_blocked_len(&doc_ids, &freqs);
        assert!(
            payload.len() < legacy,
            "le mode à exceptions doit rester sous le varint : {} contre {legacy}",
            payload.len()
        );
    }

    #[test]
    fn d2_jamais_significativement_plus_gros_que_le_varint() {
        // Propriété structurante : le mode d'un bloc est choisi par coût
        // EXACT, donc le seul surcoût possible face au format antérieur est
        // l'octet d'en-tête par bloc.
        let mut state: u32 = 0x0BAD_C0DE;
        let mut next = || -> u32 {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            state
        };
        for spread in [10u32, 1_000, 100_000, 10_000_000] {
            let mut doc_ids: Vec<u32> = (0..900).map(|_| next() % spread).collect();
            doc_ids.sort_unstable();
            doc_ids.dedup();
            let freqs: Vec<u32> = doc_ids.iter().map(|d| (d % 3) + 1).collect();
            let payload = assert_strict_round_trip(&doc_ids, &freqs);
            let legacy = legacy_blocked_len(&doc_ids, &freqs);
            let blocks = doc_ids.len().div_ceil(FOR_BLOCK_SIZE);
            assert!(
                payload.len() <= legacy + blocks,
                "spread={spread} : {} > {legacy} + {blocks}",
                payload.len()
            );
        }
    }

    #[test]
    fn d2_corpus_seede_multi_segments() {
        // Trois « segments » de bases de doc_id disjointes et croissantes,
        // comme les 12 segments du corpus de production : chacun encode et
        // relit ses propres postings sans interférence.
        let mut state: u32 = 0x5EED_1234;
        let mut next = || -> u32 {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            state
        };
        for segment in 0..3u32 {
            let base = segment * 2_400_000;
            let mut doc_ids: Vec<u32> = (0..1500).map(|_| base + next() % 2_400_000).collect();
            doc_ids.sort_unstable();
            doc_ids.dedup();
            let freqs: Vec<u32> = doc_ids.iter().map(|d| (d % 4) + 1).collect();
            assert_strict_round_trip(&doc_ids, &freqs);
        }
    }

    #[test]
    fn d2_bit_packing_round_trip_sur_toutes_les_largeurs() {
        for width in 1..=MAX_PACKED_WIDTH {
            let mask = if width == 32 {
                u32::MAX
            } else {
                (1u32 << width) - 1
            };
            let values: Vec<u32> = (0..133u32)
                .map(|i| i.wrapping_mul(2_654_435_761) & mask)
                .collect();
            let mut packed = Vec::new();
            pack_bits(&mut packed, &values, width);
            assert_eq!(packed.len(), packed_len(values.len(), width));
            let mut position = 0usize;
            let mut decoded = Vec::new();
            unpack_bits(&packed, &mut position, values.len(), width, &mut decoded).unwrap();
            assert_eq!(decoded, values, "largeur {width}");
            assert_eq!(position, packed.len());
        }
    }

    #[test]
    fn d2_bit_packing_refuse_une_entree_tronquee() {
        let values: Vec<u32> = (0..64).collect();
        let mut packed = Vec::new();
        pack_bits(&mut packed, &values, 6);
        packed.pop();
        let mut position = 0usize;
        let mut decoded = Vec::new();
        let err = unpack_bits(&packed, &mut position, values.len(), 6, &mut decoded).unwrap_err();
        assert_eq!(err, PostingsBlockError::UnexpectedEof);
    }

    #[test]
    fn d2_en_tete_a_bit_reserve_est_une_corruption() {
        let doc_ids: Vec<u32> = (0..200).map(|i| i * 3).collect();
        let freqs: Vec<u32> = vec![1; 200];
        let mut encoded = encode_postings_blocked(&doc_ids, &freqs).unwrap();
        encoded[0] |= 0b0001_0000;
        let err = decode_postings_blocked(&encoded, doc_ids.len()).unwrap_err();
        assert_eq!(err, PostingsBlockError::MalformedBlock);
    }

    #[test]
    fn d2_mode_de_canal_inconnu_est_une_corruption() {
        let doc_ids: Vec<u32> = (0..200).map(|i| i * 3).collect();
        let freqs: Vec<u32> = vec![1; 200];
        let mut encoded = encode_postings_blocked(&doc_ids, &freqs).unwrap();
        encoded[0] = (encoded[0] & !0b0000_0011) | 0b0000_0011;
        assert_eq!(
            decode_postings_blocked(&encoded, doc_ids.len()).unwrap_err(),
            PostingsBlockError::MalformedBlock
        );
    }

    #[test]
    fn d2_freqs_tres_dispersees_retombent_sur_le_varint() {
        // Repli explicite : sur un bloc minuscule à fréquences très
        // écartées, le bit-packing coûterait plus cher que le varint. Le
        // canal `freq` ne doit alors JAMAIS grossir par rapport au format
        // antérieur — c'est ce que ce mode garantit.
        let doc_ids = [4u32, 9];
        let freqs = [1u32, 300];
        let output = encode_postings_blocked_with_directory(&doc_ids, &freqs).unwrap();
        assert_eq!((output.payload[0] >> FREQ_MODE_SHIFT) & DOC_MODE_MASK, 3);
        assert_eq!(output.stats.freq_bytes, 3, "varint(1) + varint(300)");
        assert_strict_round_trip(&doc_ids, &freqs);
    }

    #[test]
    fn d2_largeur_de_bit_packing_invalide_est_une_corruption() {
        // Corpus dense : le bloc est bit-packé, donc l'octet de largeur
        // suit immédiatement le premier identifiant (ici un varint d'un
        // octet, doc_id 0).
        let doc_ids: Vec<u32> = (0..200).map(|i| i * 2).collect();
        let freqs: Vec<u32> = vec![1; 200];
        let encoded = encode_postings_blocked(&doc_ids, &freqs).unwrap();
        assert_eq!(encoded[0] & 0b0000_0011, 1, "bloc attendu bit-packé");
        assert_eq!(encoded[1], 0, "doc_id 0 sur un octet de varint");
        for bogus in [0u8, 33, 255] {
            let mut corrupted = encoded.clone();
            corrupted[2] = bogus;
            assert_eq!(
                decode_postings_blocked(&corrupted, doc_ids.len()).unwrap_err(),
                PostingsBlockError::MalformedBlock,
                "largeur {bogus}"
            );
        }
    }

    #[test]
    fn d2_freq_constante_egale_a_un_est_une_corruption() {
        // Un encodeur canonique aurait employé le mode « toutes à 1 », qui
        // n'écrit aucun octet : rencontrer la constante 1 prouve que les
        // octets ne viennent pas de cet encodeur.
        let doc_ids = [0u32, 5, 9];
        let freqs = [3u32, 3, 3];
        let mut encoded = encode_postings_blocked(&doc_ids, &freqs).unwrap();
        let last = encoded.len() - 1;
        assert_eq!(encoded[last], 3, "constante de fréquence en queue de bloc");
        encoded[last] = 1;
        assert_eq!(
            decode_postings_blocked(&encoded, doc_ids.len()).unwrap_err(),
            PostingsBlockError::MalformedBlock
        );
    }

    #[test]
    fn d2_payload_tronque_est_un_eof() {
        let doc_ids: Vec<u32> = (0..300).map(|i| i * 3).collect();
        let freqs: Vec<u32> = (0..300).map(|i| (i % 5) + 1).collect();
        let encoded = encode_postings_blocked(&doc_ids, &freqs).unwrap();
        for cut in [1usize, 2, encoded.len() / 2, encoded.len() - 1] {
            let err = decode_postings_blocked(&encoded[..cut], doc_ids.len()).unwrap_err();
            assert!(
                matches!(
                    err,
                    PostingsBlockError::UnexpectedEof | PostingsBlockError::MalformedBlock
                ),
                "coupe {cut} : {err:?}"
            );
        }
    }

    #[test]
    fn d2_repertoire_de_blocs_est_gratuit_et_exact() {
        // L'encodeur rend le répertoire en UNE passe ; il doit être
        // rigoureusement égal à celui que `block_directory` recalculait en
        // re-décodant tout le payload (le passage supprimé du chemin
        // d'indexation).
        let total = (FOR_BLOCK_SIZE * 5 + 3) as u32;
        let doc_ids: Vec<u32> = (0..total).map(|i| i * 11 + (i % 7)).collect();
        let freqs: Vec<u32> = (0..total).map(|i| (i % 3) + 1).collect();
        let output = encode_postings_blocked_with_directory(&doc_ids, &freqs).unwrap();
        assert_eq!(
            output.directory,
            block_directory(&output.payload, doc_ids.len()).unwrap()
        );
        assert_eq!(output.directory.len(), 6);
        assert_eq!(output.directory[0].byte_offset_in_term_payload, 0);
        for pair in output.directory.windows(2) {
            assert!(pair[0].byte_offset_in_term_payload < pair[1].byte_offset_in_term_payload);
            assert!(pair[0].max_doc_id < pair[1].max_doc_id);
        }
    }

    #[test]
    fn d2_compteurs_se_cumulent_sans_deborder() {
        let mut stats = BlockedEncodeStats {
            blocks: u64::MAX,
            doc_id_bytes: u64::MAX,
            freq_bytes: u64::MAX,
            freq_omitted_blocks: u64::MAX,
        };
        stats.merge(BlockedEncodeStats {
            blocks: 1,
            doc_id_bytes: 1,
            freq_bytes: 1,
            freq_omitted_blocks: 1,
        });
        assert_eq!(stats.blocks, u64::MAX);
        assert_eq!(stats.doc_id_bytes, u64::MAX);
    }

    fn skip_entries(ranges: &[(usize, u32, u32)]) -> Vec<BlockSkipEntry> {
        ranges
            .iter()
            .map(|(block_index, min_doc_id, max_doc_id)| BlockSkipEntry {
                block_index: *block_index,
                min_doc_id: *min_doc_id,
                max_doc_id: *max_doc_id,
            })
            .collect()
    }

    #[test]
    fn block_skip_list_rejects_out_of_order_indices() {
        let entries = skip_entries(&[(1, 0, 10), (0, 11, 20)]);
        let err = BlockSkipList::from_block_ranges(entries).unwrap_err();
        assert_eq!(err, PostingsBlockError::NotMonotonic);
    }

    #[test]
    fn block_skip_list_rejects_overlapping_ranges() {
        // bottom[0].max_doc_id (10) >= bottom[1].min_doc_id (10) -> reject.
        let entries = skip_entries(&[(0, 0, 10), (1, 10, 20)]);
        let err = BlockSkipList::from_block_ranges(entries).unwrap_err();
        assert_eq!(err, PostingsBlockError::NotMonotonic);
    }

    #[test]
    fn block_skip_list_rejects_inverted_min_max() {
        let entries = skip_entries(&[(0, 10, 0)]);
        let err = BlockSkipList::from_block_ranges(entries).unwrap_err();
        assert_eq!(err, PostingsBlockError::NotMonotonic);
    }

    #[test]
    fn block_skip_list_empty_top_for_single_block() {
        let entries = skip_entries(&[(0, 0, 127)]);
        let skip = BlockSkipList::from_block_ranges(entries).unwrap();
        assert_eq!(skip.entries().len(), 1);
        assert!(skip.top_layer().is_empty());
    }

    #[test]
    fn block_skip_list_builds_top_layer_every_skip_interval_blocks() {
        // 17 blocks => top layer has ceil(17 / SKIP_INTERVAL) = 3 entries
        // (indices 0, 8, 16) when SKIP_INTERVAL = 8.
        let ranges: Vec<(usize, u32, u32)> = (0..17)
            .map(|i| (i, (i as u32) * 100, (i as u32) * 100 + 99))
            .collect();
        let skip = BlockSkipList::from_block_ranges(skip_entries(&ranges)).unwrap();
        assert_eq!(skip.entries().len(), 17);
        let expected_top_len = 17_usize.div_ceil(SKIP_INTERVAL);
        assert_eq!(skip.top_layer().len(), expected_top_len);
        assert_eq!(skip.top_layer()[0].block_index, 0);
        assert_eq!(skip.top_layer()[1].block_index, SKIP_INTERVAL);
        assert_eq!(skip.top_layer()[2].block_index, 2 * SKIP_INTERVAL);
    }

    #[test]
    fn block_skip_cursor_returns_first_block_when_target_falls_inside() {
        let ranges: Vec<(usize, u32, u32)> = (0..4)
            .map(|i| (i, (i as u32) * 100, (i as u32) * 100 + 50))
            .collect();
        let skip = BlockSkipList::from_block_ranges(skip_entries(&ranges)).unwrap();
        let mut cursor = skip.cursor();
        let landed = cursor.advance_to(220).expect("doc 220 has a candidate");
        assert_eq!(landed.block_index, 2);
        assert_eq!(landed.min_doc_id, 200);
        assert_eq!(landed.max_doc_id, 250);
    }

    #[test]
    fn block_skip_cursor_returns_none_past_last_max_doc_id() {
        let ranges = vec![(0, 0_u32, 50_u32), (1, 100, 150)];
        let skip = BlockSkipList::from_block_ranges(skip_entries(&ranges)).unwrap();
        let mut cursor = skip.cursor();
        assert!(cursor.advance_to(1_000).is_none());
        assert!(cursor.advance_to(2_000).is_none());
    }

    #[test]
    fn block_skip_cursor_lands_on_next_block_when_target_in_gap() {
        // Blocks: 0..=50, 100..=150. target=75 (in the gap) should land
        // on block 1.
        let ranges = vec![(0, 0_u32, 50_u32), (1, 100, 150)];
        let skip = BlockSkipList::from_block_ranges(skip_entries(&ranges)).unwrap();
        let mut cursor = skip.cursor();
        let landed = cursor.advance_to(75).expect("doc 75 has a candidate");
        assert_eq!(landed.block_index, 1);
        assert_eq!(landed.min_doc_id, 100);
    }

    #[test]
    fn block_skip_cursor_skips_blocks_on_large_jump() {
        // Build 4 * SKIP_INTERVAL blocks of 100 doc ids each. Jump from
        // block 0 to a target in the very last block. The cursor must
        // touch FEWER than the total number of blocks: this is the
        // whole point of the top layer.
        let total_blocks = 4 * SKIP_INTERVAL;
        let ranges: Vec<(usize, u32, u32)> = (0..total_blocks)
            .map(|i| (i, (i as u32) * 100, (i as u32) * 100 + 99))
            .collect();
        let skip = BlockSkipList::from_block_ranges(skip_entries(&ranges)).unwrap();

        // Target = first doc of the last block.
        let target = ((total_blocks - 1) as u32) * 100;

        let mut cursor = skip.cursor();
        let landed = cursor.advance_to(target).expect("target found");
        assert_eq!(landed.block_index, total_blocks - 1);

        // The cursor MUST have visited strictly fewer blocks than a
        // naive linear walk would. With SKIP_INTERVAL = 8 and
        // total_blocks = 32, a linear walk touches 32 entries; the
        // top-layer-driven walk should touch <= SKIP_INTERVAL + a
        // small constant.
        assert!(
            cursor.blocks_visited() < total_blocks,
            "cursor must skip blocks: visited {} of {}",
            cursor.blocks_visited(),
            total_blocks
        );
        assert!(
            cursor.blocks_visited() <= SKIP_INTERVAL + 2,
            "cursor should touch at most SKIP_INTERVAL + 2 blocks, got {}",
            cursor.blocks_visited()
        );
    }

    #[test]
    fn block_skip_cursor_repeated_advance_keeps_monotonic_position() {
        let total_blocks = 4 * SKIP_INTERVAL;
        let ranges: Vec<(usize, u32, u32)> = (0..total_blocks)
            .map(|i| (i, (i as u32) * 100, (i as u32) * 100 + 99))
            .collect();
        let skip = BlockSkipList::from_block_ranges(skip_entries(&ranges)).unwrap();
        let mut cursor = skip.cursor();

        // Walk through targets in increasing order. After each call,
        // the position must be non-decreasing.
        let targets = [
            150_u32,
            350,
            (SKIP_INTERVAL as u32) * 100 + 50,
            (2 * SKIP_INTERVAL as u32) * 100 + 50,
            ((total_blocks - 1) as u32) * 100,
        ];
        let mut previous_position = 0;
        for &target in &targets {
            let landed = cursor.advance_to(target).expect("target found");
            assert!(landed.max_doc_id >= target);
            assert!(cursor.position() >= previous_position);
            previous_position = cursor.position();
        }
    }

    #[test]
    fn block_skip_cursor_matches_naive_linear_walk_on_seeded_corpus() {
        // Seeded corpus: 2 000 strictly increasing doc ids => ~16 blocks.
        let mut state: u32 = 0x1234_5678;
        let mut next = || -> u32 {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            state
        };
        let mut doc_ids: Vec<u32> = (0..2000).map(|_| next() % 10_000_000).collect();
        doc_ids.sort_unstable();
        doc_ids.dedup();
        let freqs: Vec<u32> = doc_ids.iter().map(|d| (d % 16) + 1).collect();
        let encoded = encode_postings_doc_id_freq(&doc_ids, &freqs).unwrap();
        let blocks = inspect_postings_blocks(&encoded).unwrap();
        let skip = BlockSkipList::from_block_metas(&blocks).unwrap();

        // Naive baseline: for every target, scan blocks linearly and
        // pick the first one whose max_doc_id >= target.
        let naive_advance = |target: u32| -> Option<usize> {
            blocks
                .iter()
                .find(|meta| meta.max_doc_id >= target)
                .map(|meta| meta.block_index)
        };

        // Drive the cursor with monotonically increasing targets so
        // we can also check that the skip-list answers match the
        // naive walk on every step.
        let targets: Vec<u32> = (0..50).map(|i| doc_ids[(i * doc_ids.len()) / 50]).collect();
        let mut cursor = skip.cursor();
        for &target in &targets {
            let expected = naive_advance(target);
            let actual = cursor.advance_to(target).map(|entry| entry.block_index);
            assert_eq!(
                actual, expected,
                "skip cursor must match naive walk at target {target}"
            );
        }
    }
}
