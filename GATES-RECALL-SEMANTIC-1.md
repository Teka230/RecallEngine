# GATES: RECALL-SEMANTIC-1 — Quality Gates & Décision Go/No-Go (Milestones P2-A, P2-B & P3)

Ce document formalise les critères d'acceptation, les résultats d'audit, la résolution des 4 points de revue technique de P1-E, l'implémentation des jalons **P2-A** (Cycle de vie & Indexeur idempotent pré-inférence), **P2-B** (Moteur Hybride RRF & Commandes CLI) et **P3** (API REST locale `/v1/search` & Visualiseur TUI interactif) et l'évaluation globale pour le chantier sémantique de [RecallEngine](README.md).

---

## 1. Grille d'Évaluation des Gates (P0 + P1 + P1-E + P2-A + P2-B + P3)

| Gate | Intitulé | Critères d'Exigence | Résultat Constaté | Statut |
| :--- | :--- | :--- | :--- | :--- |
| **G1** | **Non-régression du Coeur** | `cargo test --locked` 100 % vert. 0 régression sur les flux d'import, API, TUI, Web UI. Migrations 001 à 003 intactes. | **129 tests réussis (0 échec)** sur l'ensemble de la suite unitaire et des 18 suites d'intégration. | **VALIDÉ** |
| **G2** | **Identité Canonique** | Utiliser `content_blocks.id` (TEXT) et non `rowid` (i64). Préserver `message_id` et citation `ic` stable. | `content_block_id: String` aligné avec `content_blocks.id TEXT PRIMARY KEY`. Schéma migration 004 et requêtes alignés. | **VALIDÉ** |
| **G3** | **Mode Hors Ligne Hermétique** | Interdiction stricte de tout téléchargement réseau silencieux. Vérification complète du cache (`config.json`, `tokenizer.json`, `model.onnx`). Tests CI hermétiques. | `is_e5_small_cached()` vérifie la validité des métadonnées du modèle ONNX. Tests mock hermétiques en CI; tests FastEmbed réels gardés avec skip gracieux si le cache est absent. | **VALIDÉ (P2-A)** |
| **G4** | **Idempotence AVANT Inférence** | L'évaluation d'idempotence doit avoir lieu AVANT l'appel au modèle d'embedding. 0 appel d'inférence si le corpus est inchangé. | `SemanticIndexer` compare les empreintes composites `sha256(model_id:chunker_version:chunk_hash)` en base avant inférence. **2 ré-indexations consécutives = 0 appel, 0 embedding calculé.** | **VALIDÉ (P2-A)** |
| **G5** | **Mutation Partielle Ciblée** | La mutation d'un seul bloc ne doit recalculer les vecteurs QUE pour les chunks de ce bloc. | Testé dans `tests/semantic_lifecycle.rs` : mutation d'un bloc sur 9 entraîne exactement 1 bloc ré-indexé, 8 détectés up-to-date, et 1 seul chunk ré-embarqué. | **VALIDÉ (P2-A)** |
| **G6** | **Enregistrement Global `sqlite-vec`** | `sqlite3_auto_extension` doit être enregistré AVANT l'ouverture de toute connexion (`Database::open`, `ReadRepository::open_read_only`, `serve::open_read_only`). | `ensure_sqlite_vec_registered()` exécuté avant `Connection::open`/`open_with_flags`. Test validant `vec_version()` et les tables `vec0` sur connexions fraîches RW et RO. | **VALIDÉ (P2-A)** |
| **G7** | **Projections de Performance Corrigées** | Correction des projections logarithmiques erronées. `vec0` 0.1.x effectuant un scan exact plat SIMD $O(N)$, appliquer une extrapolation linéaire stricte et expliciter la nécessité de mesures réelles à 50k/100k chunks. | Projections mises à jour : ~90 ms @ 50k, ~180 ms @ 100k chunks. Mention explicite dans la RFC que les mesures physiques réelles sont requises avant tout engagement SLA < 50 ms. | **VALIDÉ (P2-A)** |
| **G8** | **Migration 004 & Intégrité Virtuelle** | Migration 004 déclarative avec `semantic_vec_chunks`, `semantic_vec_items USING vec0`, `semantic_index_meta` et trigger de suppression cascade. | Migration 004 ajoutée, enregistrée dans `MIGRATIONS`. Trigger `trg_semantic_vec_chunks_delete` cascade les suppressions de blocs vers la table virtuelle `vec0`. | **VALIDÉ (P2-A)** |
| **G9** | **Qualité du Code & Lints** | `cargo fmt --check` et `cargo clippy --locked --all-targets --all-features -- -D warnings` sans aucune erreur. | Formatage 100 % conforme. Clippy 100 % vert avec zéro warning sous `-D warnings`. | **VALIDÉ (P2-A)** |
| **G10** | **Algorithme Hybride RRF & Boosting** | Fusion des scores FTS5 (BM25) et KNN (`sqlite-vec`) selon $1/(k + rank)$ ($k=60.0$). Les matches doubles doivent obtenir un score supérieur aux matches simples. | Testé dans `src/semantic/hybrid.rs` et `tests/semantic_hybrid.rs`. Match double FTS5+KNN classé en tête devant les matches simples. | **VALIDÉ (P2-B)** |
| **G11** | **Repli Gracieux FTS5 (Graceful Fallback)** | En l'absence d'index vectoriel ou si le modèle n'est pas disponible, bascule automatique sur FTS5 sans crash ni blocage. | Testé dans `test_hybrid_fallback_fts5_when_unindexed` : `mode` renvoie `"hybrid-fallback-fts5"`, les résultats et totaux exacts sont préservés. | **VALIDÉ (P2-B)** |
| **G12** | **CLI `recall embed`** | Indexation par lots (`--batch-size`), mode hors ligne strict par défaut, détection du cache, sortie standard claire et sortie JSON (`--json`). | Testé de bout en bout dans `tests/semantic_embed_cli.rs`. Deuxième exécution via CLI confirme 0 recalcul (idempotence pré-inférence). | **VALIDÉ (P2-B)** |
| **G13** | **CLI `recall search --mode`** | Support des modes `lexical`, `semantic`, `hybrid` avec filtres (`--role`, `--ic-min`, etc.) et enveloppe JSON standard. | Testé dans `tests/semantic_embed_cli.rs`. Enveloppes JSON valides, mode sémantique rejetant proprement les bases non indexées. | **VALIDÉ (P2-B)** |
| **G14** | **Recherche Conceptuelle Hybride** | Retrouver des messages pertinents sur des requêtes où FTS5 renvoie 0 résultat en raison de divergences de vocabulaire. | Testé dans `test_hybrid_conceptual_retrieval_fastembed` : requête sur l'autorité de l'agent retrouvée en Top-1 (IC 1) alors que FTS5 renvoie 0. | **VALIDÉ (P2-B)** |
| **G15** | **API REST Locale `/v1/search?mode=...`** | Paramètre `mode` (`lexical`, `semantic`, `hybrid`) dans l'API REST avec curseur paginé et conservation stricte du contrat de réponse. | Testé dans `tests/read_api_v1.rs`. Repli FTS5 automatique en mode hybride sans index, 400 Bad Request explicite en mode sémantique sans index. | **VALIDÉ (P3)** |
| **G16** | **Visualiseur TUI Terminal (`recall browse`)** | Recherche hybride par défaut sous `/`, avec bascule dynamique en direct par `[Tab]` (`hybrid` ↔ `lexical` ↔ `semantic`) et rafraîchissement d'overlay. | Testé unitairement dans `src/tui/tests.rs` (`test_tui_submit_search_and_mode_toggle`) et visuellement dans `src/tui/overlays.rs`. | **VALIDÉ (P3)** |

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

## 4. Synthèse des Implémentations Appliquées en P2-B

### (1) Moteur Hybride RRF (`src/semantic/hybrid.rs`)
- Implémentation du service `HybridSearchService` avec paramètres configurables `RrfParams` (par défaut $k=60.0$, $w_{\text{fts}}=1.0$, $w_{\text{vec}}=1.0$).
- Pool de candidats élargi à $(limit + offset) \times 3$ (minimum 50) pour maximiser le rappel avant fusion.
- Dédoublonnage vectoriel au niveau message : regroupement des chunks par `message_id` en retenant la distance minimale avant attribution du rang vectoriel.
- Tri déterministe (`rrf_score DESC`, puis `created_at ASC`, puis `message_id ASC`).
- Conversion transparente en `SearchMatch` canonique avec conservation des citations `ic`, des snippets et des horodatages.
- Repli gracieux automatique vers FTS5 (`hybrid-fallback-fts5`) en l'absence d'index vectoriel ou si aucun modèle n'est fourni.

### (2) Commande CLI `recall embed` (`src/commands/embed.rs`)
- Arguments supportés : `--db`, `--batch-size` (défaut 32), `--prune`, `--allow-download`, `--cache-dir`, `--json`.
- Contrôle strict hors ligne par défaut (`StrictOffline`) : refuse de s'exécuter si les poids ONNX ne sont pas présents localement, sauf avec `--allow-download`.
- Rapport console détaillé (durée, vitesse en chunks/s, blocs indexés, blocs à jour) et format JSON via `JsonEnvelope`.

### (3) Commande CLI `recall search --mode` (`src/commands/search.rs`)
- Ajout du drapeau `--mode <lexical|semantic|hybrid>` (défaut : `lexical`).
- Dispatching vers `SearchService` (lexical FTS5), `HybridSearchService` (hybride RRF) ou extraction vectorielle pure (sémantique KNN).
- Garde-fous explicites : message clair si l'utilisateur demande `--mode semantic` sur une base non indexée.

---

## 5. Synthèse des Implémentations Appliquées en P3

### (1) API REST Locale `GET /v1/search?mode=...` (`src/commands/read_api_v1.rs`)
- Paramètre query `mode` optionnel (`lexical`, `semantic`, `hybrid`, défaut `lexical`).
- Dispatching vers `ReadRepository::search_page_with_mode` avec instanciation du provider FastEmbed si le cache local est présent.
- Repli gracieux automatique : en mode `hybrid`, bascule silencieuse et transparente sur `search_page_fts5` si la base n'est pas indexée ou si le cache du modèle n'est pas disponible.
- Gestion d'erreur HTTP rigoureuse : renvoi de `400 Bad Request` en mode `semantic` explicite si la base n'est pas indexée (`semantic index not initialized. Run 'recall embed' first`), évitant toute erreur 500 inattendue.
- Pagination par curseur préservée pour tous les modes (`next_cursor`), format de réponse JSON `PageResponse<SearchResponse>` strictement compatible avec les consommateurs existants (Web UI, SDKs, scripts).

### (2) Visualiseur TUI Terminal `recall browse` (`src/tui/`)
- Recherche hybride activée par défaut lors de la soumission d'une requête avec `/`.
- Bascule interactive en direct via `[Tab]` dans l'overlay de résultats (`Overlay::SearchResults`) : alterne dynamiquement entre `hybrid`, `lexical` et `semantic` avec ré-exécution immédiate de la recherche sans quitter l'overlay.
- Indicateur de mode temps réel dans le header de l'overlay (`Search (hybrid) · "query" · Tab mode · Enter open`).
### (3) Consolidation Finale & Alignement Global (P3-Consolidation)
- **Dédoublonnage CLI sémantique** : Pool de candidats élargi (`fetch_limit = max(50, (limit + offset) * 3)`) et dédoublonnage strict par `message_id` dans `recall search --mode semantic` (`src/commands/search.rs`), aligné avec le comportement de `ReadRepository` et du moteur hybride.
- **Support effectif de `--prune` et métadonnées** : Configuration de l'élagage via `SemanticIndexer::with_prune(options.prune)` et persistance de la métadonnée d'horodatage `last_indexed_at` dans `semantic_index_meta`.
- **Filtres d'intégrité `is_active = 1`** : Filtrage systématique des messages et conversations actifs (`m.is_active = 1 AND c.is_active = 1`) lors de l'indexation (`fetch_eligible_blocks`), de la recherche vectorielle (`fetch_vector_candidates`) et de la recherche FTS5 (`build_where_clause` et `count_sql`).
- **Interface Web React (`web/`)** : Intégration d'un sélecteur à 3 modes (`Hybrid` / `Lexical` / `Semantic`) dans la barre de recherche globale et affichage du mode actif dans le panneau des résultats avec ré-exécution immédiate lors du changement de mode.
- **Documentation publique** : Mise à jour de `README.md` (retrait des mentions obsolètes "no embeddings", documentation de `recall embed`, `recall search --mode`, TUI et Web UI) et de `READ-API-V1.md` (documentation de `GET /v1/search?mode=...`).

---

## 6. Décision Finale : GO GLOBAL VALIDÉ SANS RÉSERVE — Initiative RECALL-SEMANTIC-1 Complétée

### **DÉCISION : GO VALIDÉ SANS RÉSERVE POUR L'ENSEMBLE DU CHANTIER RECALL-SEMANTIC-1 (P0 → P3)**

Toutes les exigences architecturales, fonctionnelles, de performance et de qualité de code définies dans la RFC ont été satisfaites :

- **P0 + P1** : Étude comparative rigoureuse (FastEmbed, `multilingual-e5-small`, `sqlite-vec` vs BLOB), fixtures anonymisées, benchmarks M2.
- **P1-E** : Résolution des 4 points bloquants (identité canonique `content_blocks.id`, mode hors ligne hermétique, déduplication stricte par chunk/bloc, requêtes paramétrées).
- **P2-A** : Migration 004 déclarative, cycle de vie vectoriel, triggers d'intégrité, indexation incrémentale pré-inférence (0 recalcul sur corpus inchangé), enregistrement global `sqlite-vec`.
- **P2-B** : Algorithme hybride RRF ($k=60.0$) avec pool élargi et boosting des correspondances duales, commandes CLI `recall embed` et `recall search --mode`.
- **P3** : Intégration de bout en bout dans l'API REST locale (`GET /v1/search?mode=...`) et dans le visualiseur TUI interactif (`recall browse`) avec bascule `[Tab]`.
- **Indicateurs de Qualité Globale** :
  - **129 tests réussis** sur l'ensemble des suites unitaires et d'intégration (`cargo test --locked`).
  - **0 warning clippy** sous `-D warnings`.
  - Formatage 100 % conforme (`cargo fmt --check`).
  - 100 % hors ligne par défaut, 0 fuite réseau, aucune régression sur les migrations 001 à 003 ni sur les flux d'ingestion/TUI/Web UI existants.

