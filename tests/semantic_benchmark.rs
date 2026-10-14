use recall_engine::semantic::chunking::{chunk_content_block, ChunkerOptions, TextChunk};
use recall_engine::semantic::embedding::{
    EmbeddingProvider, FastEmbedProvider, MockDeterministicProvider,
};
use recall_engine::semantic::store::{SqliteBlobStore, SqliteVecStore, VectorStore};
use rusqlite::Connection;
use std::fs;
use std::path::Path;
use std::time::Instant;

fn get_db_file_size(path: &Path, conn: &Connection) -> u64 {
    // Flush any pending WAL pages to ensure file size reflects full database
    let _ = conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);");
    let mut total = fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    // Include WAL / SHM if present
    let wal = path.with_extension("sqlite-wal");
    if wal.exists() {
        total += fs::metadata(&wal).map(|m| m.len()).unwrap_or(0);
    }
    let shm = path.with_extension("sqlite-shm");
    if shm.exists() {
        total += fs::metadata(&shm).map(|m| m.len()).unwrap_or(0);
    }
    total
}

#[test]
fn benchmark_m2_semantic_pipeline() {
    recall_engine::semantic::store::ensure_sqlite_vec_registered();

    println!("\n==========================================================================");
    println!("   RECALL-SEMANTIC-1 : Apple Silicon M2 Physical On-Disk Scaling Benchmark");
    println!("==========================================================================\n");

    // 1. Inférence locale FastEmbed M2 (Micro-benchmarks sur batch 1, 8, 32)
    println!("--- 1. INFERENCE LOCALE FASTEMBED (Multilingual E5 Small / 384 dim) ---");
    let mut embedder = FastEmbedProvider::try_new(None).expect("Init FastEmbed M2");

    let sample_text = "RecallEngine preserves the conversation graph, message IDs, source JSON, and branch structure in a canonical local database.";
    let start_single = Instant::now();
    let _ = embedder.embed_passages(&[sample_text]).unwrap();
    let single_latency = start_single.elapsed();
    println!("   - Latence unitaire (batch 1) : {:?}", single_latency);

    let batch8_texts = vec![sample_text; 8];
    let start_b8 = Instant::now();
    let _ = embedder.embed_passages(&batch8_texts).unwrap();
    let b8_latency = start_b8.elapsed();
    println!(
        "   - Latence batch 8            : {:?} ({:?}/chunk)",
        b8_latency,
        b8_latency / 8
    );

    let batch32_texts = vec![sample_text; 32];
    let start_b32 = Instant::now();
    let _ = embedder.embed_passages(&batch32_texts).unwrap();
    let b32_latency = start_b32.elapsed();
    println!(
        "   - Latence batch 32           : {:?} ({:?}/chunk)",
        b32_latency,
        b32_latency / 32
    );
    println!(
        "   - Throughput batch 32        : {:.1} chunks/s\n",
        32.0 / b32_latency.as_secs_f64()
    );

    // 2. Synthetic corpus generator
    let sample_paragraphs = [
        "RecallEngine preserves the conversation graph, message IDs, source JSON, and branch structure in a canonical local database.",
        "Le protocole d'Internal Citations garantit que chaque message actif recoit un entier unique et stable preservable lors des reimports.",
        "Pour separer l'autorite de la proposition, un mutation guard verifie les actions de l'agent avant toute emission vers le socket unix.",
        "SQLite FTS5 tokenize avec unicode61 remove_diacritics 2 pour permettre la recherche plein texte rapide avec scoring BM25.",
        "Les modeles d'embeddings denses comme Multilingual E5 Small capturent la proximite semantique independamment des mots exacts.",
        "Dans une architecture locale, les donnees de sante et les identifiants personnels ne doivent jamais quitter la machine hote.",
        "La synchronisation des branches et la resolution de contexte s'effectuent a travers une fenetre deterministe par IC.",
        "L'optimisation des performances SQLite necessite d'ajuster le cache_size et de surveiller la contention lors des transactions.",
    ];

    let chunker_opts = ChunkerOptions::default();
    let generate_dataset = |count: usize| -> (Vec<TextChunk>, Vec<Vec<f32>>) {
        let mut chunks = Vec::with_capacity(count);
        for i in 0..count {
            let text = format!(
                "[{i}] {} - Chunking benchmark metadata payload {i}.",
                sample_paragraphs[i % sample_paragraphs.len()]
            );
            let block_id = format!("cb-bench-{i}");
            let msg_id = format!("msg-bench-{i}");
            let block_chunks =
                chunk_content_block(&block_id, &msg_id, Some(i as i64 + 1), &text, &chunker_opts);
            chunks.extend(block_chunks);
        }
        chunks.truncate(count);

        let mut mock = MockDeterministicProvider::default();
        let texts: Vec<&str> = chunks.iter().map(|c| c.text.as_str()).collect();
        let vectors = mock.embed_passages(&texts).unwrap();
        (chunks, vectors)
    };

    // 3. Multi-Tier Scaling Benchmark (On-Disk Real Measurements)
    println!("--- 2. MESURES REELLES SUR DISQUE & MULTI-TIER SCALING ---");
    let tiers = [100, 500, 2000, 5000];

    #[derive(Debug)]
    struct TierMetrics {
        count: usize,
        vec_db_bytes: u64,
        blob_db_bytes: u64,
        vec_bytes_per_chunk: f64,
        blob_bytes_per_chunk: f64,
        vec_insert_ms: f64,
        blob_insert_ms: f64,
        vec_knn_top10_us: u128,
        blob_knn_top10_us: u128,
        speedup: f64,
    }

    let mut metrics = Vec::new();
    let mut mock = MockDeterministicProvider::default();
    let query_vector = mock
        .embed_query("Architecture et gouvernance des agents")
        .unwrap();

    for &tier_count in &tiers {
        let tmp = tempfile::tempdir().unwrap();
        let vec_db_path = tmp.path().join("vec_benchmark.sqlite");
        let blob_db_path = tmp.path().join("blob_benchmark.sqlite");

        let vec_conn = Connection::open(&vec_db_path).unwrap();
        let blob_conn = Connection::open(&blob_db_path).unwrap();

        let vec_store = SqliteVecStore::new(&vec_conn, 384);
        let blob_store = SqliteBlobStore::new(&blob_conn, 384);

        vec_store.create_tables().unwrap();
        blob_store.create_tables().unwrap();

        let (chunks, vectors) = generate_dataset(tier_count);

        // Benchmark insertions
        let start_vec_insert = Instant::now();
        for (c, v) in chunks.iter().zip(vectors.iter()) {
            vec_store.insert_chunk(c, v).unwrap();
        }
        let vec_insert_ms = start_vec_insert.elapsed().as_secs_f64() * 1000.0;

        let start_blob_insert = Instant::now();
        for (c, v) in chunks.iter().zip(vectors.iter()) {
            blob_store.insert_chunk(c, v).unwrap();
        }
        let blob_insert_ms = start_blob_insert.elapsed().as_secs_f64() * 1000.0;

        // Measure physical on-disk file sizes
        let vec_db_bytes = get_db_file_size(&vec_db_path, &vec_conn);
        let blob_db_bytes = get_db_file_size(&blob_db_path, &blob_conn);

        // Measure KNN @ Top-10
        let iters = 15;
        let start_vec_knn = Instant::now();
        for _ in 0..iters {
            let res = vec_store.search_knn(&query_vector, 10).unwrap();
            assert_eq!(res.len(), 10.min(tier_count));
        }
        let vec_knn_top10_us = start_vec_knn.elapsed().as_micros() / iters;

        let start_blob_knn = Instant::now();
        for _ in 0..iters {
            let res = blob_store.search_knn(&query_vector, 10).unwrap();
            assert_eq!(res.len(), 10.min(tier_count));
        }
        let blob_knn_top10_us = start_blob_knn.elapsed().as_micros() / iters;

        let speedup = (blob_knn_top10_us as f64) / (vec_knn_top10_us as f64).max(1.0);

        metrics.push(TierMetrics {
            count: tier_count,
            vec_db_bytes,
            blob_db_bytes,
            vec_bytes_per_chunk: (vec_db_bytes as f64) / (tier_count as f64),
            blob_bytes_per_chunk: (blob_db_bytes as f64) / (tier_count as f64),
            vec_insert_ms,
            blob_insert_ms,
            vec_knn_top10_us,
            blob_knn_top10_us,
            speedup,
        });
    }

    // Display measured table
    println!(
        "{:<6} | {:<12} | {:<12} | {:<12} | {:<12} | {:<14} | {:<14} | {:<7}",
        "Chunks",
        "vec0 Disk",
        "BLOB Disk",
        "vec0 Ins",
        "BLOB Ins",
        "vec0 Top10(µs)",
        "BLOB Top10(µs)",
        "Speedup"
    );
    println!("---------------------------------------------------------------------------------------------------------");
    for m in &metrics {
        println!(
            "{:<6} | {:>9.1} KB | {:>9.1} KB | {:>9.1} ms | {:>9.1} ms | {:>10} µs   | {:>10} µs   | {:>5.1}x",
            m.count,
            (m.vec_db_bytes as f64) / 1024.0,
            (m.blob_db_bytes as f64) / 1024.0,
            m.vec_insert_ms,
            m.blob_insert_ms,
            m.vec_knn_top10_us,
            m.blob_knn_top10_us,
            m.speedup
        );
    }

    // 4. Extrapolation to 50,000 and 100,000 chunks
    let last = metrics.last().unwrap();
    let vec_kb_per_chunk = last.vec_bytes_per_chunk / 1024.0;
    let blob_kb_per_chunk = last.blob_bytes_per_chunk / 1024.0;
    let blob_us_per_chunk = (last.blob_knn_top10_us as f64) / (last.count as f64);
    let vec_us_5k = last.vec_knn_top10_us as f64;

    println!("\n--- 3. EXTRAPOLATION A GRANDE ECHELLE (50 000 & 100 000 CHUNKS) ---");
    println!(
        "Base empirique calculee sur le tier {} chunks :",
        last.count
    );
    println!("   - sqlite-vec disque : {:.2} KB/chunk", vec_kb_per_chunk);
    println!("   - BLOB disque       : {:.2} KB/chunk", blob_kb_per_chunk);
    println!(
        "   - BLOB scan rate    : {:.3} µs/chunk (O(N) brute force)",
        blob_us_per_chunk
    );

    for target in [50_000, 100_000] {
        let proj_vec_mb = (target as f64 * vec_kb_per_chunk) / 1024.0;
        let proj_blob_mb = (target as f64 * blob_kb_per_chunk) / 1024.0;
        let proj_blob_latency_ms = (target as f64 * blob_us_per_chunk) / 1000.0;
        // sqlite-vec indexation/SIMD sublinear scaling factor approximation
        let proj_vec_latency_ms = (vec_us_5k * ((target as f64 / 5000.0).log2() + 1.0)) / 1000.0;

        println!("\n   => Projection pour {} chunks :", target);
        println!("      * Taille disque sqlite-vec : ~{:.1} MB", proj_vec_mb);
        println!("      * Taille disque BLOB       : ~{:.1} MB", proj_blob_mb);
        println!(
            "      * Latence KNN sqlite-vec   : ~{:.1} ms",
            proj_vec_latency_ms
        );
        println!(
            "      * Latence KNN BLOB         : ~{:.1} ms (Inacceptable pour UI temps reel)",
            proj_blob_latency_ms
        );
        println!(
            "      * Facteur d'acceleration : ~{:.0}x",
            proj_blob_latency_ms / (proj_vec_latency_ms / 1000.0).max(0.001) / 1000.0
        );
    }

    println!("\n==========================================================================\n");
}
