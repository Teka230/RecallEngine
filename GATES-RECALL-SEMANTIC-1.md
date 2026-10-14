# GATES: RECALL-SEMANTIC-1 — Quality Gates & Décision Go/No-Go (Milestone P2-A)

Ce document formalise les critères d'acceptation, les résultats d'audit, la résolution des 4 points de revue technique de P1-E, l'implémentation du jalon **P2-A** (Cycle de vie & Indexeur idempotent pré-inférence) et l'évaluation pour le chantier sémantique de [RecallEngine](README.md).

---

## 1. Grille d'Évaluation des Gates (P0 + P1 + P1-E + P2-A)

| Gate | Intitulé | Critères d'Exigence | Résultat Constaté | Statut |
| :--- | :--- | :--- | :--- | :--- |
| **G1** | **Non-régression du Coeur** | `cargo test --locked` 100 % vert. 0 régression sur les flux d'import, API, TUI, Web UI. Migrations 001 à 003 intactes. | **118+ tests réussis (0 échec)** sur l'ensemble de la suite unitaire et des 16 suites d'intégration. | **VALIDÉ** |
| **G2** | **Identité Canonique** | Utiliser `content_blocks.id` (TEXT) et non `rowid` (i64). Préserver `message_id` et citation `ic` stable. | `content_block_id: String` aligné avec `content_blocks.id TEXT PRIMARY KEY`. Schéma migration 004 et requêtes alignés. | **VALIDÉ** |
| **G3** | **Mode Hors Ligne Hermétique** | Interdiction stricte de tout téléchargement réseau silencieux. Vérification complète du cache (`config.json`, `tokenizer.json`, `model.onnx`). Tests CI hermétiques. | `is_e5_small_cached()` vérifie la validité des métadonnées du modèle ONNX. Tests mock hermétiques en CI; tests FastEmbed réels gardés avec skip gracieux si le cache est absent. | **VALIDÉ (P2-A)** |
| **G4** | **Idempotence AVANT Inférence** | L'évaluation d'idempotence doit avoir lieu AVANT l'appel au modèle d'embedding. 0 appel d'inférence si le corpus est inchangé. | `SemanticIndexer` compare les empreintes composites `sha256(model_id:chunker_version:chunk_hash)` en base avant inférence. **2 ré-indexations consécutives = 0 appel, 0 embedding calculé.** | **VALIDÉ (P2-A)** |
| **G5** | **Mutation Partielle Ciblée** | La mutation d'un seul bloc ne doit recalculer les vecteurs QUE pour les chunks de ce bloc. | Testé dans `tests/semantic_lifecycle.rs` : mutation d'un bloc sur 9 entraîne exactement 1 bloc ré-indexé, 8 détectés up-to-date, et 1 seul chunk ré-embarqué. | **VALIDÉ (P2-A)** |
| **G6** | **Enregistrement Global `sqlite-vec`** | `sqlite3_auto_extension` doit être enregistré AVANT l'ouverture de toute connexion (`Database::open`, `ReadRepository::open_read_only`, `serve::open_read_only`). | `ensure_sqlite_vec_registered()` exécuté avant `Connection::open`/`open_with_flags`. Test validant `vec_version()` et les tables `vec0` sur connexions fraîches RW et RO. | **VALIDÉ (P2-A)** |
| **G7** | **Projections de Performance Corrigées** | Correction des projections logarithmiques erronées. `vec0` 0.1.x effectuant un scan exact plat SIMD $O(N)$, appliquer une extrapolation linéaire stricte et expliciter la nécessité de mesures réelles à 50k/100k chunks. | Projections mises à jour : ~90 ms @ 50k, ~180 ms @ 100k chunks. Mention explicite dans la RFC que les mesures physiques réelles sont requises avant tout engagement SLA < 50 ms. | **VALIDÉ (P2-A)** |
| **G8** | **Migration 004 & Intégrité Virtuelle** | Migration 004 déclarative avec `semantic_vec_chunks`, `semantic_vec_items USING vec0`, `semantic_index_meta` et trigger de suppression cascade. | Migration 004 ajoutée, enregistrée dans `MIGRATIONS`. Trigger `trg_semantic_vec_chunks_delete` cascade les suppressions de blocs vers la table virtuelle `vec0`. | **VALIDÉ (P2-A)** |
| **G9** | **Qualité du Code & Lints** | `cargo fmt --check` et `cargo clippy --locked --all-targets --all-features -- -D warnings` sans aucune erreur. | Formatage 100 % conforme. Clippy 100 % vert avec zéro warning sous `-D warnings`. | **VALIDÉ (P2-A)** |

---

## 2. Synthèse des 4 Correctifs Structurants Appliqués en P2-A

### (1) Projections de Performance $O(N)$ Linéaires pour `sqlite-vec`
- **Constat** : La version 0.1.x de `sqlite-vec` implémente un balayage exact accéléré par SIMD (NEON sur ARM / AVX sur x86), et non une structure d'indexation sous-linéaire (ANN / HNSW).
- **Correction** : Remplacement de l'extrapolation logarithmique par une projection linéaire $O(N)$ à partir des mesures physiques réelles à 5 000 chunks (9,0 ms) :
  - **50 000 chunks** : ~90 ms (extrapolation indicative).
  - **100 000 chunks** : ~180 ms (extrapolation indicative).
- **Directive** : Inscription formelle dans la RFC et les gates : aucune promesse de SLA < 50 ms à 100k chunks ne sera gravée avant des mesures physiques sur banc de test dédié à 50k et 100k vecteurs.

### (2) Sécurisation Hors Ligne et Herméticité des Tests
- **Constat** : `is_e5_small_cached()` ne vérifiait pas la validité du fichier ONNX (pouvant être un symlink brisé dans le cache Hugging Face). Par ailleurs, les tests d'intégration FastEmbed supposaient la présence locale du cache.
- **Correction** :
  - `is_e5_small_cached()` valide désormais l'existence et la lisibilité physique (`std::fs::metadata`) de `onnx/model.onnx` ou `model.onnx`.
  - Ajout du test hermétique `test_is_e5_small_cached_hermetic_structure` utilisant `tempfile`.
  - Les tests FastEmbed lourds (`semantic_spike`, `semantic_benchmark`) vérifient `if !is_e5_small_cached(None) { return; }` pour s'exécuter gracieusement sans tentative de téléchargement en environnement CI neuf.

### (3) Déplacement de l'Idempotence AVANT l'Inférence (`SemanticIndexer`)
- **Constat** : Dans le spike initial, les embeddings étaient calculés avant l'appel à `sync_block_chunks()`. Le stockage pouvait renvoyer `UpToDate`, mais le modèle avait déjà consommé du temps CPU/GPU inutilement.
- **Correction** :
  - Création de `SemanticIndexer::index_all()`.
  - Pour chaque bloc éligible, le chunking déterministe génère une empreinte composite `fingerprint = sha256(model_id:chunker_version:chunk_hash)`.
  - Cette empreinte est comparée aux lignes existantes dans `semantic_vec_chunks` **avant tout appel au modèle**.
  - Si l'empreinte et le nombre de chunks correspondent : le bloc est marqué `UpToDate` et **aucun embedding n'est calculé**.
  - Seuls les chunks des blocs nouveaux ou modifiés sont agrégés dans le lot passé à `embed_passages()`.
  - **Preuve vérifiée par test** (`tests/semantic_lifecycle.rs`) :
    - Passe 1 (neuf) : 9 blocs indexés, 9 embeddings calculés (1 batch call).
    - Passe 2 (immédiate) : 9 blocs up-to-date, 0 bloc indexé, 0 embedding calculé (**0 appel d'inférence**).
    - Passe 3 (1 bloc muté) : 8 blocs up-to-date, 1 bloc indexé, 1 embedding calculé (**inférence strictement limitée au delta**).

### (4) Initialisation de `sqlite-vec` AVANT l'Ouverture de Connexion
- **Constat** : `sqlite3_auto_extension()` ne s'applique qu'aux connexions SQLite ouvertes *après* son enregistrement.
- **Correction** :
  - `ensure_sqlite_vec_registered()` est appelé de manière systématique au tout début de `Database::open()`, `ReadRepository::open_read_only()`, et `commands::serve::open_read_only()`.
  - Fourniture de `register_sqlite_vec_on_connection(&Connection)` pour enregistrer explicitement l'extension sur des connexions déjà actives si nécessaire.
  - Test d'intégration dédié `test_sqlite_vec_registered_on_fresh_connections` validant l'accès immédiat à `vec_version()` et aux tables virtuelles `vec0` dès l'ouverture des connexions.

---

## 3. Synthèse de la Migration 004 & Gestion du Cycle de Vie

La migration déclarative `004_semantic_search.sql` introduit :
1. `semantic_vec_chunks` : Métadonnées canoniques des chunks (`chunk_id`, `content_block_id`, `message_id`, `ic`, `chunk_index`, `total_chunks`, `chunker_version`, `model_id`, `chunk_hash`, `fingerprint`, `text`, `created_at`).
2. `semantic_vec_items USING vec0` : Table virtuelle vectorielle 384 dimensions cosinus.
3. `semantic_index_meta` : Clé-valeur d'état de l'index (`model_id`, `chunker_version`, `dimension`, `total_indexed_chunks`, `last_indexed_at`).
4. Trigger `trg_semantic_vec_chunks_delete` : Garantit la purge atomique de `semantic_vec_items` lors de toute suppression (cascade ou manuelle) dans `semantic_vec_chunks`.

---

## 4. Décision d'Étape : GO P2-A VALIDÉ — Feu Vert pour P2-B

### **DÉCISION : GO VALIDÉ POUR P2-A (PRÊT POUR P2-B)**

### Périmètre verrouillé pour P2-A :
- Aucune exposition utilisateur prématurée (pas de commande CLI finale, pas de RRF, pas d'API `/v1/search?mode=hybrid`, pas de TUI).
- Fondations d'indexation incrémentale, d'enregistrement d'extension et de persistance 100 % opérationnelles et testées.

### Prochaine étape recommandée (P2-B) :
- Implémentation de la commande CLI `recall embed --db <path> [--batch-size 32] [--prune]`.
- Implémentation de l'algorithme de fusion de rangs réciproques (**RRF**) combinant FTS5 BM25 et KNN vectoriel.
