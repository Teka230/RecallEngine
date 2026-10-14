# GATES: RECALL-SEMANTIC-1 — Quality Gates & Décision Go/No-Go

Ce document formalise les critères d'acceptation, les résultats d'audit, l'analyse des risques et la décision de passage en phase P2 pour le chantier sémantique de [RecallEngine](README.md).

---

## 1. Grille d'Évaluation des Gates (P0 + P1)

| Gate | Intitulé | Critères d'Exigence | Résultat Constaté | Statut |
| :--- | :--- | :--- | :--- | :--- |
| **G1** | **Non-régression du Coeur** | `cargo test --locked` 100 % vert. 0 régression sur les flux d'import, API, TUI, Web UI. Migrations 001, 002, 003 intactes. | **114 tests réussis (0 échec)** sur l'ensemble de la suite unitaire et des 15 suites d'intégration. | **VALIDÉ** |
| **G2** | **Traçabilité & Déterminisme** | Le chunking doit préserver `content_block_id`, `message_id`, `ic` et produire des hashes SHA-256 reproductibles. | `test_deterministic_multi_chunk_with_overlap` validé. 100 % d'accord sur les IC. | **VALIDÉ** |
| **G3** | **Herméticité & Offline** | Inférence locale sans réseau. Possibilité d'exécuter des tests sans téléchargement de modèle. | FastEmbed opère sur cache local hors ligne (`~/.fastembed_cache`). `MockDeterministicProvider` intégré pour tests hermétiques. | **VALIDÉ** |
| **G4** | **Performance Apple Silicon M2** | Throughput batch > 50 chunks/s. Latence de requête KNN < 5 ms. Empreinte disque < 3 KB/chunk. | **91.1 chunks/s** (batch 32). Requête KNN à **0.90 ms** (< 1 ms). Empreinte **1.56 KB/chunk**. | **VALIDÉ** |
| **G5** | **Gain Sémantique vs FTS5** | La recherche sémantique doit réussir sur des requêtes conceptuelles où FTS5 échoue (0 résultat). | Requête sur les permissions des agents : **FTS5 = 0 résultat**, **FastEmbed/sqlite-vec = Top-1 exact** (distance 0.121). | **VALIDÉ** |
| **G6** | **Qualité du Code & Lints** | `cargo fmt --check` et `cargo clippy --locked --all-targets --all-features -- -D warnings` sans aucune erreur. | Formatage strict respecté. Clippy 100 % au vert (0 warning). | **VALIDÉ** |

---

## 2. Analyse des Risques & Mitigations

### Risque 1 : Dépendance C native et linking statique (`sqlite-vec`)
- *Description* : `sqlite-vec` est une extension C compilée via `cc`. Elle nécessite une compatibilité stricte avec `libsqlite3-sys` / `rusqlite` sans conflit de symboles.
- *Constat du Spike* : La compilation statique (`SQLITE_CORE`) et l'auto-extension `sqlite3_auto_extension(sqlite3_vec_init)` fonctionnent parfaitement sur macOS ARM64 sans aucun binaire dynamique externe.
- *Mitigation P2* : Encapsuler l'initialisation dans `ensure_sqlite_vec_registered()` dès l'ouverture de la connexion pour garantir l'activation transparente de `vec0`.

### Risque 2 : Poids du runtime ONNX (`ort`) et espace disque de compilation
- *Description* : Le runtime ONNX et les métadonnées de debug complètes peuvent générer des binaires lourds (> 200 Mo par binaire de test) risquant de saturer le disque sur des partitions restreintes.
- *Constat du Spike* : La configuration `[profile.test] debug = 1` dans `Cargo.toml` réduit drastiquement la taille des binaires de test (divisée par 10) tout en conservant la traçabilité des lignes d'erreur.
- *Mitigation P2* : Conserver `profile.test debug = 1` et documenter l'utilisation du cache `.fastembed_cache` (isolé dans `.gitignore`).

### Risque 3 : Dérive de l'index vectoriel lors d'un ré-import
- *Description* : En cas de ré-import d'un export ChatGPT enrichi, les nouveaux messages doivent être indexés sans recalculer l'intégralité du corpus.
- *Constat du Spike* : Grâce à `chunk_hash` (SHA-256) et à la clé primaire `content_block_id`, l'idempotence peut être garantie par un simple `LEFT JOIN` excluant les chunks déjà présents.
- *Mitigation P2* : La commande `recall embed` devra adopter un pattern incrémental strict : `WHERE content_block_id NOT IN (SELECT content_block_id FROM semantic_vec_chunks)`.

---

## 3. Décision Finale : GO / NO-GO pour la Phase P2

### **DÉCISION : GO (Favorable sans réserve)**

### Rationale :
1. **Validation technique indiscutable** : `sqlite-vec` délivre une recherche vectorielle cosinus en **0.90 ms** (13x plus rapide que le scan BLOB), tout en restant encapsulée dans le fichier SQLite local unique.
2. **Qualité de restitution** : La supériorité sur FTS5 pour la recherche d'intention/concept est démontrée empiriquement sur fixture réelle.
3. **Zéro impact sur l'existant** : Le projet compile sans aucun warning, les 114 tests existants restent intacts, et les flux d'import, d'API et de TUI n'ont subi aucune modification.
4. **Feu vert pour P2** : Les prérequis sont réunis pour implémenter la commande CLI d'indexation incrémentale (`recall embed`) et le moteur de fusion hybride RRF.
