# RFC: RECALL-SEMANTIC-1 — Local Semantic Search & Hybrid Retrieval

- **Status** : P3 COMPLETED (Recherche Hybride dans l'API REST locale & Visualiseur TUI interactif)
- **Author** : Teka230 & AI Pair Programmer
- **Date** : 2026-10-15
- **Branch** : `feat/semantic-search` (base: `ece4555`, migrations 001 à 003 préservées)
- **Scope** : Tranche P0 + P1 + P1-E + P2-A + P2-B + P3 (Moteur hybride, CLI, API REST /v1/search et TUI)

---

## 1. Contexte et Problématique

[RecallEngine](README.md) importe des exports ChatGPT vers une base SQLite canonique locale en garantissant :
1. Une traçabilité totale via les identifiants durables **IC (Internal Citations)** et UUIDs sources.
2. Une conservation fidèle du graphe des conversations et de leurs branches.
3. Une recherche plein texte reposant sur SQLite FTS5 (tokenisation `unicode61`, scoring BM25) et un repli `LIKE`.

### La limite constatée
La recherche lexicale FTS5 excelle sur les termes exacts (identifiants, noms de classes, fonctions, citations textuelles), mais échoue structurellement sur les requêtes conceptuelles :
- *Exemple testé* : `Comment réguler les permissions de l'agent sans lui donner les pleins pouvoirs ?`
- *Résultat FTS5* : **0 résultat**, car le message source utilise des concepts synonymes (*« mutation guard »*, *« registre de contraintes hôte »*, *« séparer l'autorité de la proposition »*) sans contenir les mots exacts de la question.

L'objectif de **RECALL-SEMANTIC-1** est d'ajouter une brique de **recherche sémantique dense locale**, souveraine, sans dépendance cloud ni serveur externe, pour permettre la recherche hybride (FTS5 + Vecteurs) en préservant strictement l'intégrité canonique.

---

## 2. Principes Fondateurs & Invariants

1. **Local-first & Vie Privée Absolue** : Inférence 100 % locale sur machine hôte (Apple Silicon M2 / CPU NEON). Mode `StrictOffline` par défaut qui vérifie le cache local (`.fastembed_cache`) et interdit tout téléchargement réseau silencieux.
2. **Index Dérivé Non Destructif** : Les tables vectorielles sont des projections reconstructibles. La base canonique (`conversations`, `messages`, `content_blocks`, migrations 001 à 003) reste la seule source de vérité.
3. **Identité et Traçabilité Canonique** : Chaque chunk vectorisé est rattaché à son `content_block_id` textuel (`content_blocks.id TEXT PRIMARY KEY`), son `message_id` (UUID) et son citation `ic` stable. L'utilisation d'identifiants éphémères (`cb.rowid`) est strictement proscrite.
4. **Idempotence AVANT Inférence** : L'état du corpus est comparé aux métadonnées vectorielles existantes *avant* d'invoquer le modèle d'embedding. Un corpus inchangé engendre **zéro** calcul de vecteur et **zéro** appel d'inférence. Seuls les blocs nouveaux ou modifiés sont embarqués.
5. **Initialisation Proactive de `sqlite-vec`** : L'extension `sqlite-vec` est enregistrée globalement avant l'ouverture de toute connexion SQLite applicative (`Database::open`, `ReadRepository::open_read_only`, `serve::open_read_only`).
6. **Herméticité des Tests** : Présence d'un `MockDeterministicProvider` avec comptabilisation atomique des appels pour valider le cycle d'indexation en CI sans dépendance au téléchargement d'un modèle lourd.
7. **Tolérance aux Pannes & Repli Gracieux** : Si l'index vectoriel est absent ou vide, le moteur bascule gracieusement en mode dégradé FTS5 (`hybrid-fallback-fts5`) sans lever d'erreur ni bloquer l'utilisateur.

---

## 3. Architecture Technique

### 3.1. Modèle d'Embedding : `intfloat/multilingual-e5-small`
- **Taille** : ~133 Mo (poids ONNX quantifiés/float32).
- **Dimensionnalité** : 384 dimensions.
- **Multilinguisme** : Support natif haute fidélité du Français et de l'Anglais.
- **Spécificité asymétrique E5** :
  - Préfixe document : `passage: <texte>`
  - Préfixe requête : `query: <texte>`
- **Runtime** : `fastembed` (Rust natif via ONNX Runtime `ort`).
- **Garde-fou Offline** : `OfflineMode::StrictOffline` vérifiant la présence et la validité physique des fichiers `tokenizer.json`, `config.json` et `model.onnx` (y compris cibles de symlinks) avant instanciation.

### 3.2. Découpage Déterministe & Empreinte Composite
Le module `src/semantic/chunking.rs` découpe les blocs textuels de façon stable et reproductible :
- **Taille maximale de chunk** : 512 caractères (~100 tokens).
- **Recouvrement (Overlap)** : 64 caractères (évite la rupture de sens aux frontières de paragraphes/phrases).
- **Frontières naturelles** : Découpage prioritaire sur les doubles sauts de ligne (`\n\n`), puis la ponctuation (`. `, `! `, `? `), puis les espaces.
- **Empreinte Composite (Fingerprint)** :
  $$\text{fingerprint} = \text{sha256}(\text{model\_id} : \text{chunker\_version} : \text{chunk\_hash})$$
  Cette empreinte garantit que si le modèle ou la stratégie de chunking change, l'invalidation et la ré-indexation sont automatiques.

### 3.3. Schéma de Stockage Vectoriel & Migration 004
La migration `004_semantic_search.sql` structure la persistance vectorielle :

```sql
CREATE TABLE IF NOT EXISTS semantic_vec_chunks (
  chunk_id           INTEGER PRIMARY KEY AUTOINCREMENT,
  content_block_id   TEXT NOT NULL REFERENCES content_blocks(id) ON DELETE CASCADE,
  message_id         TEXT NOT NULL REFERENCES messages(id) ON DELETE CASCADE,
  ic                 INTEGER,
  chunk_index        INTEGER NOT NULL,
  total_chunks       INTEGER NOT NULL,
  chunker_version    TEXT NOT NULL DEFAULT 'v1-512-64',
  model_id           TEXT NOT NULL DEFAULT 'intfloat/multilingual-e5-small',
  chunk_hash         TEXT NOT NULL,
  fingerprint        TEXT NOT NULL,
  text               TEXT NOT NULL,
  created_at         TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
  UNIQUE (content_block_id, chunk_index)
);

CREATE INDEX IF NOT EXISTS idx_vec_chunks_block ON semantic_vec_chunks(content_block_id);
CREATE INDEX IF NOT EXISTS idx_vec_chunks_hash ON semantic_vec_chunks(chunk_hash);
CREATE INDEX IF NOT EXISTS idx_vec_chunks_fingerprint ON semantic_vec_chunks(fingerprint);
CREATE INDEX IF NOT EXISTS idx_vec_chunks_message ON semantic_vec_chunks(message_id);
CREATE INDEX IF NOT EXISTS idx_vec_chunks_ic ON semantic_vec_chunks(ic);

CREATE VIRTUAL TABLE IF NOT EXISTS semantic_vec_items USING vec0(
  chunk_id INTEGER PRIMARY KEY,
  embedding float[384] distance_metric=cosine
);

CREATE TABLE IF NOT EXISTS semantic_index_meta (
  key        TEXT PRIMARY KEY,
  value      TEXT NOT NULL,
  updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);

CREATE TRIGGER IF NOT EXISTS trg_semantic_vec_chunks_delete
AFTER DELETE ON semantic_vec_chunks
BEGIN
  DELETE FROM semantic_vec_items WHERE chunk_id = OLD.chunk_id;
END;
```

### 3.4. Indexeur Idempotent : `SemanticIndexer`
Le composant `SemanticIndexer` (`src/semantic/indexer.rs`) pilote le cycle d'indexation incrémentale :
1. **Extraction éligible** : Récupère les blocs textuels des messages actifs de rôles `user` et `assistant`.
2. **Chunking & Détection d'invalidation** : Compare les fingerprints des chunks calculés avec les lignes existantes en base.
3. **Inférence ciblée** : N'invoque `embedder.embed_passages()` que sur les chunks des blocs nouveaux ou modifiés, par lots configurables (`--batch-size`, défaut 32).
4. **Synchronisation atomique** : Insère ou remplace les vecteurs par bloc via un `SAVEPOINT`.
5. **Élagage (Pruning)** : Supprime de l'index les blocs devenus inéligibles (rôle modifié, texte vidé ou bloc supprimé).
6. **Mise à jour des métadonnées** : Actualise `model_id`, `chunker_version`, `dimension`, `total_indexed_chunks`, et `last_indexed_at` dans `semantic_index_meta`.

### 3.5. Moteur de Recherche Hybride : Reciprocal Rank Fusion (RRF)
Le module `src/semantic/hybrid.rs` fusionne les résultats issus du moteur lexical FTS5 (BM25) et du moteur sémantique dense `sqlite-vec` (KNN cosinus) :

1. **Formule RRF** :
   $$\text{Score}_{\text{RRF}}(d) = \frac{w_{\text{fts}}}{k + \text{rank}_{\text{fts}}(d)} + \frac{w_{\text{vec}}}{k + \text{rank}_{\text{vec}}(d)}$$
   avec la constante de lissage standard $k = 60.0$, $w_{\text{fts}} = 1.0$, et $w_{\text{vec}} = 1.0$.

2. **Dédoublonnage au niveau Message** :
   La recherche vectorielle renvoie des correspondances au niveau chunk. Si plusieurs chunks d'un même message figurent dans les candidats KNN, le message ne figure qu'une seule fois dans la liste vectorielle, avec le meilleur rang (distance minimale).

3. **Pool de Candidats Élargi** :
   Pour chaque modalité (FTS5 et KNN), le moteur interroge un pool élargi :
   $$\text{pool\_size} = \max(50, (\text{limit} + \text{offset}) \times 3)$$
   afin de garantir un rappel optimal avant la fusion.

4. **Filtrage et Tri Déterministe** :
   Les filtres canoniques (`role`, `ic_min`, `ic_max`, `date_min`, `date_max`) s'appliquent de manière uniforme sur les deux canaux. Le tri final s'effectue par :
   `rrf_score DESC`, puis `created_at ASC`, puis `message_id ASC`.

5. **Repli Gracieux (Graceful Fallback)** :
   Si la table vectorielle n'est pas initialisée ou si aucun modèle n'est disponible, `HybridSearchService` bascule automatiquement sur une recherche lexicale FTS5 standard avec `mode: "hybrid-fallback-fts5"`.

---

## 4. Exposition CLI : `recall embed` et `recall search --mode`

### 4.1. Commande `recall embed`
Génère ou met à jour l'index vectoriel sémantique dans la base SQLite canonique :
```bash
recall embed --db <path.sqlite> [OPTIONS]
```
- `--batch-size <N>` : Taille des lots d'inférence (défaut : 32).
- `--prune` : Élaguer les blocs orphelins ou devenus inéligibles.
- `--allow-download` : Autoriser le téléchargement initial du modèle depuis Hugging Face (par défaut : `StrictOffline`).
- `--cache-dir <DIR>` : Répertoire personnalisé pour le cache modèle.
- `--json` : Émission d'une enveloppe JSON standard (`JsonEnvelope<IndexingStats>`) pour automatisation et scripts.

### 4.2. Commande `recall search --mode`
Extension de la commande `recall search` avec l'option `--mode <lexical|semantic|hybrid>` (défaut : `lexical`) :
- `lexical` : Recherche textuelle pure FTS5 (BM25).
- `semantic` : Recherche vectorielle dense pure via `sqlite-vec`.
- `hybrid` : Recherche hybride RRF combinant BM25 et KNN vectoriel.

---

## 5. Résultats Empiriques & Analyse de Performance (Apple Silicon M2)

### 5.1. Micro-benchmarks Inférence FastEmbed (M2)
- **Latence unitaire (batch 1)** : ~31.1 ms
- **Latence batch 8** : ~55.0 ms (6.88 ms / chunk)
- **Latence batch 32** : ~217.9 ms (**6.81 ms / chunk**, soit **146.9 chunks/s**)

### 5.2. Multi-Tier Scaling mesuré sur disque physique

| Chunks | `sqlite-vec` Disque | BLOB Disque | `sqlite-vec` Top-10 | BLOB Top-10 | Speedup KNN |
| :--- | :--- | :--- | :--- | :--- | :--- |
| **100** | 1 632 KB | 232 KB | 635 µs (0.64 ms) | 1 817 µs (1.82 ms) | **2.9x** |
| **500** | 1 808 KB | 1 088 KB | 1 164 µs (1.16 ms) | 8 762 µs (8.76 ms) | **7.5x** |
| **2 000** | 4 004 KB (4.0 MB) | 4 300 KB (4.3 MB) | 3 708 µs (3.71 ms) | 43 886 µs (43.9 ms) | **11.8x** |
| **5 000** | 9 904 KB (9.9 MB) | 10 676 KB (10.7 MB) | 9 019 µs (9.02 ms) | 99 639 µs (99.6 ms) | **11.0x** |

### 5.3. Projections Linéaires $O(N)$
`vec0` en version 0.1.x implémente une recherche vectorielle exacte par balayage plat (flat scan) accéléré par instructions vectorielles SIMD (ARM NEON / AVX). Complexité strictement linéaire $O(N)$ :
- **50 000 chunks** : Taille disque `sqlite-vec` ~96.7 MB, latence KNN ~90 ms.
- **100 000 chunks** : Taille disque `sqlite-vec` ~193.4 MB, latence KNN ~180 ms.

### 5.4. Validation Sémantique vs FTS5 & Hybride RRF
Sur la requête conceptuelle :
> *« Comment réguler les permissions de l'agent sans lui donner les pleins pouvoirs ? »*
- **FTS5** : 0 match.
- **`sqlite-vec` (FastEmbed)** : Message cible retrouvé en **Top-1** (`00000000-0000-4000-8000-000000000101`, IC:1) avec une distance cosinus de **0.121**.
- **Recherche Hybride RRF** : Message cible retrouvé en **Top-1** avec score RRF validé, préservant la citation canonique `IC:1` et son horodatage.

---

## 6. Intégration Applicative (P3)

### 6.1. API REST Locale : `GET /v1/search?q=...&mode=...`
L'API REST en lecture seule (`src/commands/read_api_v1.rs`) supporte le paramètre `mode` :
- `GET /v1/search?q=query&mode=lexical` : Recherche lexicale FTS5 standard (défaut).
- `GET /v1/search?q=query&mode=hybrid` : Recherche hybride RRF combinant BM25 et KNN vectoriel, avec repli gracieux automatique vers FTS5 si l'index n'est pas encore généré ou si le cache est absent.
- `GET /v1/search?q=query&mode=semantic` : Recherche vectorielle dense pure via KNN cosinus. Renvoie `400 Bad Request` si la base n'est pas indexée (`Run 'recall embed' first`).
- **Compatibilité ascendante** : La forme de réponse `PageResponse<SearchResponse>` reste strictement identique et interopérable avec les clients et l'interface Web existants.
- **Pagination par curseur** : Support transparent du curseur paginé `next_cursor` sur les résultats hybrides et sémantiques.

### 6.2. Visualiseur TUI Terminal : `recall browse`
L'interface TUI interactive (`src/tui/`) intègre nativement la recherche hybride :
- **Recherche par défaut** : L'appui sur `/` déclenche par défaut une recherche hybride enrichie, exploitant l'index vectoriel s'il est disponible.
- **Bascule interactive `[Tab]`** : Dans l'overlay des résultats de recherche (`Overlay::SearchResults`), appuyer sur `[Tab]` bascule instantanément entre `hybrid`, `lexical` et `semantic` avec rafraîchissement immédiat des résultats et affichage du mode actif dans le titre de l'overlay (`Search (hybrid) · "query" · Tab mode · Enter open`).
- **Robustesse** : Si la base n'a pas été indexée avec `recall embed`, le mode hybride bascule gracieusement sur les résultats lexicaux sans interruption de l'expérience utilisateur.

