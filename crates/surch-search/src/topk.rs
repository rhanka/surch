//! L'ordre du top-K scoré, et le SEUIL COMPÉTITIF qui en découle.
//!
//! Chantier C1. Ce module ne contient qu'une seule chose : la règle de
//! départage du top-K, et l'unique conséquence qu'on a le droit d'en tirer
//! pour élaguer. Les deux vivent ici, ensemble, pour qu'il soit impossible
//! de faire dériver l'une sans l'autre — le piège exact de ce moteur, où le
//! corpus deces rend PRESQUE TOUS LES SCORES D'UN TERME ÉGAUX (`tf = 1` et
//! `doc_len = 1` sur le champ `NOM` analysé en `norm`), donc où la règle de
//! départage n'est pas un cas limite mais le cas NOMINAL.

use std::cmp::Ordering;

/// LE comparateur du top-K scoré : score décroissant, puis `doc_id`
/// croissant pour départager les ex æquo.
///
/// Défini **une seule fois**, ici, et partagé par tous les collecteurs
/// (`surch-api` le ré-exporte). Comme les `doc_id` sont uniques, cet ordre
/// est TOTAL : deux entrées distinctes ne comparent jamais `Equal`, donc le
/// résultat d'un top-K borné ne dépend pas de l'ordre d'arrivée des
/// candidats.
pub fn scored_pair_ordering(a: &(f64, u32), b: &(f64, u32)) -> Ordering {
    b.0.partial_cmp(&a.0)
        .unwrap_or(Ordering::Equal)
        .then_with(|| a.1.cmp(&b.1))
}

/// Seuil compétitif VIVANT : le score du K-ième meilleur document déjà
/// collecté, quand K documents l'ont été.
///
/// # Pourquoi la comparaison est STRICTE
///
/// C'est le seul point délicat, et il est entièrement dicté par
/// [`scored_pair_ordering`]. Soit `(worst_score, worst_id)` la K-ième
/// meilleure entrée déjà collectée, et `(s, d)` un candidat ULTÉRIEUR.
///
/// Les postings sont parcourus en `doc_id` **strictement croissant** (dans
/// un segment par construction du codec, entre segments parce que les
/// `doc_base` sont croissants et que chaque segment ne couvre que sa propre
/// plage). Donc `d > worst_id`, TOUJOURS.
///
/// Un collecteur borné n'accepte `(s, d)` que si
/// `scored_pair_ordering(&(s, d), &(worst_score, worst_id)) == Less`, soit :
///
/// - `s > worst_score` — le candidat gagne sur le score ; ou
/// - `s == worst_score` **et** `d < worst_id` — impossible ici.
///
/// L'admission se réduit donc EXACTEMENT à `s > worst_score`. Un ex æquo
/// avec le K-ième perd, et tout document dont on sait majorer le score par
/// `worst_score` peut être écarté **sans changer une seule ligne du
/// résultat** : ni les documents rendus, ni leur ordre, ni leurs scores.
///
/// C'est le pendant du `Math.nextUp(pqTop.score)` de Lucene, obtenu ici
/// sans `nextUp` puisque le départage par `doc_id` croissant suffit.
///
/// # Ce que ce type n'autorise PAS
///
/// Il ne dit rien du `total` (`hits.total`) : écarter un document du top-K
/// ne le retire pas du comptage. Un appelant qui compte doit continuer à
/// compter.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct MinCompetitiveScore {
    kth_best: Option<f64>,
}

impl MinCompetitiveScore {
    /// Seuil non encore armé : moins de K documents collectés, donc tout
    /// candidat est compétitif.
    pub const fn unset() -> Self {
        Self { kth_best: None }
    }

    /// Arme (ou relève) le seuil sur le score du K-ième meilleur document
    /// collecté. Le seuil d'un collecteur borné ne peut que croître, mais
    /// ce type ne l'impose pas : il enregistre ce qu'on lui donne, et c'est
    /// l'appelant qui garantit lire son K-ième réel.
    pub fn observe_kth_best(&mut self, kth_best_score: f64) {
        self.kth_best = Some(kth_best_score);
    }

    /// `true` si un document dont le score est **majoré** par
    /// `score_upper_bound` peut encore entrer dans le top-K.
    ///
    /// `score_upper_bound` peut être le score exact du document (élagage
    /// par document) ou n'importe quel majorant prouvé (borne de bloc, borne
    /// de terme). Dans les deux cas la comparaison est stricte, cf. la
    /// démonstration portée par ce type.
    pub fn admits(&self, score_upper_bound: f64) -> bool {
        match self.kth_best {
            None => true,
            Some(kth_best) => score_upper_bound > kth_best,
        }
    }

    /// `true` dès qu'un seuil est armé. Sert UNIQUEMENT à court-circuiter le
    /// calcul d'un majorant : tant que le seuil n'est pas armé, [`Self::admits`]
    /// répond `true` quel que soit son argument, donc calculer ce majorant
    /// serait du travail pur perdu sur le chemin chaud. La règle d'admission
    /// elle-même reste entièrement dans [`Self::admits`] — ce prédicat ne la
    /// duplique pas.
    pub fn is_armed(&self) -> bool {
        self.kth_best.is_some()
    }

    /// Valeur courante du seuil, pour l'observabilité et les tests.
    pub fn kth_best(&self) -> Option<f64> {
        self.kth_best
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Réplique minimale de la règle d'admission d'un collecteur borné
    /// (`surch-api::topn::TopN::push` : on n'insère que sur `Less` strict).
    fn collecteur_admet(candidat: (f64, u32), pire_retenu: (f64, u32)) -> bool {
        scored_pair_ordering(&candidat, &pire_retenu) == Ordering::Less
    }

    #[test]
    fn le_seuil_non_arme_admet_tout() {
        let seuil = MinCompetitiveScore::unset();
        assert!(seuil.admits(f64::NEG_INFINITY));
        assert!(seuil.admits(0.0));
        assert_eq!(seuil.kth_best(), None);
    }

    #[test]
    fn admits_coincide_avec_le_collecteur_sur_des_doc_id_croissants() {
        // Le seul régime dans lequel `admits` est utilisé : le candidat
        // arrive APRÈS le pire retenu, donc avec un `doc_id` plus grand.
        let pire_retenu = (1.5_f64, 100_u32);
        let mut seuil = MinCompetitiveScore::unset();
        seuil.observe_kth_best(pire_retenu.0);

        for candidat_score in [0.0_f64, 1.499_999_999, 1.5, 1.500_000_001, 3.0] {
            for doc_id in [101_u32, 200, u32::MAX] {
                assert_eq!(
                    seuil.admits(candidat_score),
                    collecteur_admet((candidat_score, doc_id), pire_retenu),
                    "divergence seuil/collecteur pour ({candidat_score}, {doc_id})"
                );
            }
        }
    }

    #[test]
    fn is_armed_ne_dit_rien_de_plus_que_admits() {
        let mut seuil = MinCompetitiveScore::unset();
        assert!(!seuil.is_armed());
        assert!(seuil.admits(f64::NEG_INFINITY), "non armé : tout passe");
        seuil.observe_kth_best(1.0);
        assert!(seuil.is_armed());
        assert!(!seuil.admits(1.0));
    }

    #[test]
    fn un_ex_aequo_ulterieur_est_rejete() {
        // Le cas NOMINAL du corpus deces : tous les scores égaux.
        let pire_retenu = (2.0_f64, 42_u32);
        let mut seuil = MinCompetitiveScore::unset();
        seuil.observe_kth_best(pire_retenu.0);
        assert!(!seuil.admits(2.0));
        assert!(!collecteur_admet((2.0, 43), pire_retenu));
        // ... alors que le MÊME score avec un `doc_id` plus petit gagnerait :
        // c'est pourquoi la strictesse n'est licite que sur un parcours
        // ascendant.
        assert!(collecteur_admet((2.0, 41), pire_retenu));
    }

    #[test]
    fn un_ulp_au_dessus_reste_competitif() {
        let pire_retenu = (2.0_f64, 42_u32);
        let mut seuil = MinCompetitiveScore::unset();
        seuil.observe_kth_best(pire_retenu.0);
        let juste_au_dessus = f64::from_bits(2.0_f64.to_bits() + 1);
        assert!(seuil.admits(juste_au_dessus));
        assert!(collecteur_admet((juste_au_dessus, 43), pire_retenu));
    }

    #[test]
    fn l_ordre_est_total_sur_des_doc_id_uniques() {
        let paires = [(1.0_f64, 1_u32), (1.0, 2), (2.0, 1), (2.0, 2)];
        for (i, a) in paires.iter().enumerate() {
            for (j, b) in paires.iter().enumerate() {
                assert_eq!(
                    scored_pair_ordering(a, b) == Ordering::Equal,
                    i == j,
                    "seules deux entrées identiques peuvent comparer Equal"
                );
            }
        }
    }
}
