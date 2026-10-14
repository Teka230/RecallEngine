# RFC: RECALL-SEMANTIC-1 — Local Semantic Search & Hybrid Retrieval

- **Status** : PROPOSED / SPIKE COMPLETED (P0 + P1)
- **Author** : Teka230 & AI Pair Programmer
- **Date** : 2026-10-14
- **Branch** : `feat/semantic-search` (base: `ece4555`, migration 003 préservée)
- **Scope** : Tranche P0 + P1 (Spike technique isolé, sans mutation de l'import, API, TUI ou Web UI)

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

1. **Local-first & Vie Privée Absolue** : Inférence 100 % locale sur machine hôte (Apple Silicon M2 / CPU NEON). Aucun texte ni vecteur n'est envoyé sur le réseau.
2. **Index Dérivé Non Destructif** : Les tables vectorielles sont des projections reconstructibles. La base canonique (`conversations`, `messages`, `content_blocks`, migrations 001 à 003) reste la seule source de vérité.
3. **Traçabilité Canonique** : Chaque chunk vectorisé est rattaché à son `content_block_id`, `message_id` et son citation `ic` stable.
4. **Herméticité des Tests** : Présence d'un `MockDeterministicProvider` permettant de tester la tuyauterie vectorielle en CI/offline sans dépendre du téléchargement d'un modèle lourd.
5. **Isolation Stricte P0/P1** : Zéro modification des commandes d'importation (`recall import`), de l'API HTTP (`recall serve`), de la TUI (`recall browse`) ou de l'interface Web.

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

### 3.2. Découpage Déterministe (Chunking)
Le module `src/semantic/chunking.rs` découpe les blocs textuels de façon stable et reproductible :
- **Taille maximale de chunk** : 512 caractères (~100 tokens).
- **Recouvrement (Overlap)** : 64 caractères (évite la rupture de sens aux frontières de paragraphes/phrases).
- **Frontières naturelles** : Découpage prioritaire sur les doubles sauts de ligne (`\n\n`), puis la ponctuation (`. `, `! `, `? `), puis les espaces.
- **Empreinte SHA-256** : Chaque chunk calcule son `chunk_hash = sha256(trimmed_text)` pour permettre la déduplication et l'idempotence.

### 3.3. Comparatif de Stockage Vectoriel : `sqlite-vec` vs SQLite BLOB
Deux moteurs de persistance vectorielle ont été implémentés et benchmarqués dans `src/semantic/store.rs` :

1. **`SqliteVecStore` (`sqlite-vec` v0.1.9)** :
   - Extension SQLite officielle en C compilée statiquement avec le binaire Rust (`SQLITE_CORE`).
   - Table virtuelle `vec0` avec métrique cosinus native :
     ```sql
     CREATE VIRTUAL TABLE IF NOT EXISTS semantic_vec_items USING vec0(
         chunk_id INTEGER PRIMARY KEY,
         embedding float[384] distance_metric=cosine
     );
     ```
   - Requête KNN par correspondance native :
     ```sql
     SELECT chunk_id, distance
     FROM semantic_vec_items
     WHERE embedding MATCH ?1 AND k = ?2
     ORDER BY distance ASC;
     ```

2. **`SqliteBlobStore` (Baseline BLOB standard)** :
   - Table SQLite standard stockant le vecteur brut en `BLOB` IEEE-754 (384 * 4 = 1536 octets).
   - Distance cosinus calculée par scan en mémoire Rust ($1.0 - \vec{a} \cdot \vec{b}$).

---

## 4. Résultats Empiriques sur Apple Silicon M2

Les benchmarks ont été exécutés sur la machine locale (MacBook Pro M2, macOS) via `tests/semantic_benchmark.rs` sur 500 chunks synthétiques et `tests/semantic_spike.rs` sur les fixtures réelles :

| Métrique | `sqlite-vec` (`vec0`) | SQLite BLOB standard | Gain / Ratio |
| :--- | :--- | :--- | :--- |
| **Inférence unitaire (batch 1)** | 22.4 ms | N/A | Modèle local M2 |
| **Inférence batched (batch 32)** | **10.9 ms / chunk** (91.1 chunks/s) | N/A | Idéal pour l'indexation |
| **Débit de chunking** | 64 334 chunks/s | 64 334 chunks/s | Instantané |
| **Vitesse d'insertion (500 v)** | 21 251 vec/s (23.5 ms) | 75 534 vec/s (6.6 ms) | BLOB plus rapide à l'écriture |
| **Empreinte disque** | **1.56 KB / chunk** | 1.62 KB / chunk | Quasi équivalent |
| **Recherche KNN Top-1** | **902 µs (0.90 ms)** | 11 629 µs (11.63 ms) | **12.9x plus rapide** |
| **Recherche KNN Top-5** | **918 µs (0.92 ms)** | 11 476 µs (11.48 ms) | **12.5x plus rapide** |
| **Recherche KNN Top-10** | **891 µs (0.89 ms)** | 11 216 µs (11.22 ms) | **12.6x plus rapide** |
| **Parité du Top-1 (Accuracy)** | **100 % d'accord** | **100 % d'accord** | Parité mathématique stricte |
| **Chevauchement Top-5** | **100 %** | **100 %** | Parité de classement totale |

### Validation Sémantique vs FTS5
Sur la requête conceptuelle :
> *« Comment réguler les permissions de l'agent sans lui donner les pleins pouvoirs ? »*
- **FTS5** : 0 match.
- **`sqlite-vec` (FastEmbed)** : Message cible retrouvé en **Top-1** (`00000000-0000-4000-8000-000000000101`, IC:1) avec une distance cosinus de **0.121** (extrêmement proche).

---

## 5. Synthèse & Plan pour P2

Le spike démontre que **`sqlite-vec` surclasse le stockage BLOB standard par un facteur 12.5x à 13x en latence de requête sous la milliseconde (< 1 ms)**, tout en conservant une taille disque identique (~1.56 KB/vecteur).

La tranche P2 pourra introduire :
1. Une commande dédiée `recall embed --db <path> [--batch-size 32]`.
2. Le moteur de fusion **RRF (Reciprocal Rank Fusion)** combinant BM25 FTS5 et similarité vectorielle `sqlite-vec`.
3. L'intégration transparente dans `/v1/search` et `recall browse`.
