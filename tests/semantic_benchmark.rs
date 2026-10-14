use recall_engine::semantic::chunking::{chunk_content_block, ChunkerOptions};
use recall_engine::semantic::embedding::{
    EmbeddingProvider, FastEmbedProvider, MockDeterministicProvider,
};
use recall_engine::semantic::store::{SqliteBlobStore, SqliteVecStore, VectorStore};
use rusqlite::Connection;
use std::time::Instant;

#[test]
fn benchmark_m2_semantic_pipeline() {
    recall_engine::semantic::store::ensure_sqlite_vec_registered();

    println!("\n=======================================================");
    println!("   RECALL-SEMANTIC-1 : Apple Silicon M2 Benchmark");
    println!("=======================================================\n");

    let conn = Connection::open_in_memory().unwrap();
    let dimension = 384;
    let vec_store = SqliteVecStore::new(&conn, dimension);
    let blob_store = SqliteBlobStore::new(&conn, dimension);

    vec_store.create_tables().unwrap();
    blob_store.create_tables().unwrap();

    // 1. Synthetic corpus generation (representing real ChatGPT conversation chunks)
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

    let target_chunk_count = 500;
    let mut chunks = Vec::with_capacity(target_chunk_count);
    let chunker_opts = ChunkerOptions::default();

    let start_chunking = Instant::now();
    for i in 0..target_chunk_count {
        let text = format!(
            "[{}] {} - Details additionnels sur le bloc {} pour tester le chunking deterministe.",
            i,
            sample_paragraphs[i % sample_paragraphs.len()],
            i
        );
        let block_chunks = chunk_content_block(
            i as i64,
            &format!("msg-{i}"),
            Some(i as i64 + 1),
            &text,
            &chunker_opts,
        );
        chunks.extend(block_chunks);
    }
    let chunking_duration = start_chunking.elapsed();
    let total_chunks = chunks.len();

    println!("1. CHUNKING DETERMINISTE");
    println!("   - Chunks generes       : {}", total_chunks);
    println!("   - Duree totale         : {:?}", chunking_duration);
    println!(
        "   - Debit chunking       : {:.0} chunks/s\n",
        (total_chunks as f64) / chunking_duration.as_secs_f64()
    );

    // 2. Inférence locale FastEmbed M2 (Micro-benchmarks sur batch 1, 8, 32)
    println!("2. INFERENCE LOCALE FASTEMBED (Multilingual E5 Small / 384 dim)");
    let mut embedder = FastEmbedProvider::try_new(None).expect("Init FastEmbed M2");

    // Single item latency (batch 1)
    let start_single = Instant::now();
    let _ = embedder.embed_passages(&[chunks[0].text.as_str()]).unwrap();
    let single_latency = start_single.elapsed();
    println!("   - Latence unitaire (batch 1) : {:?}", single_latency);

    // Batch 8 latency
    let batch8_texts: Vec<&str> = chunks.iter().take(8).map(|c| c.text.as_str()).collect();
    let start_b8 = Instant::now();
    let _ = embedder.embed_passages(&batch8_texts).unwrap();
    let b8_latency = start_b8.elapsed();
    println!(
        "   - Latence batch 8            : {:?} ({:?}/chunk)",
        b8_latency,
        b8_latency / 8
    );

    // Batch 32 latency
    let batch32_texts: Vec<&str> = chunks.iter().take(32).map(|c| c.text.as_str()).collect();
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

    // 3. Ingestion & Stockage (sqlite-vec vs BLOB) sur le dataset
    println!(
        "3. STOCKAGE & INSERTION (sur {} vecteurs 384-dim)",
        total_chunks
    );

    // Using Mock for mass insertion to test DB engine throughput without waiting on 500x model inferences
    let mut mock = MockDeterministicProvider::default();
    let texts: Vec<&str> = chunks.iter().map(|c| c.text.as_str()).collect();
    let all_vectors = mock.embed_passages(&texts).unwrap();

    let start_vec_insert = Instant::now();
    for (chunk, vec) in chunks.iter().zip(all_vectors.iter()) {
        vec_store.insert_chunk(chunk, vec).unwrap();
    }
    let vec_insert_duration = start_vec_insert.elapsed();

    let start_blob_insert = Instant::now();
    for (chunk, vec) in chunks.iter().zip(all_vectors.iter()) {
        blob_store.insert_chunk(chunk, vec).unwrap();
    }
    let blob_insert_duration = start_blob_insert.elapsed();

    println!(
        "   - Insertion sqlite-vec (vec0): {:?} ({:.0} vec/s)",
        vec_insert_duration,
        (total_chunks as f64) / vec_insert_duration.as_secs_f64()
    );
    println!(
        "   - Insertion SQLite BLOB      : {:?} ({:.0} vec/s)",
        blob_insert_duration,
        (total_chunks as f64) / blob_insert_duration.as_secs_f64()
    );

    let vec_bytes = total_chunks * (dimension * 4 + 64);
    let blob_bytes = total_chunks * (dimension * 4 + 128);
    println!(
        "   - Empreinte approx sqlite-vec: {:.1} KB ({:.2} KB/chunk)",
        (vec_bytes as f64) / 1024.0,
        (vec_bytes as f64) / (total_chunks as f64 * 1024.0)
    );
    println!(
        "   - Empreinte approx BLOB      : {:.1} KB ({:.2} KB/chunk)\n",
        (blob_bytes as f64) / 1024.0,
        (blob_bytes as f64) / (total_chunks as f64 * 1024.0)
    );

    // 4. Recherche KNN (Top-1, Top-5, Top-10)
    println!("4. RECHERCHE KNN & LATENCE DE REQUETE");
    let query_vec = mock
        .embed_query("Architecture et gouvernance des agents")
        .unwrap();

    for k in [1, 5, 10] {
        // Warmup & 10 iterations measure
        let iters = 20;
        let start_vec_k = Instant::now();
        for _ in 0..iters {
            let res = vec_store.search_knn(&query_vec, k).unwrap();
            assert_eq!(res.len(), k);
        }
        let avg_vec_us = start_vec_k.elapsed().as_micros() / iters;

        let start_blob_k = Instant::now();
        for _ in 0..iters {
            let res = blob_store.search_knn(&query_vec, k).unwrap();
            assert_eq!(res.len(), k);
        }
        let avg_blob_us = start_blob_k.elapsed().as_micros() / iters;

        println!("   - KNN @ Top-{:<2} | sqlite-vec: {:>5} µs ({:.2} ms) | SQLite BLOB: {:>5} µs ({:.2} ms) | Gain: {:.1}x",
            k,
            avg_vec_us,
            (avg_vec_us as f64) / 1000.0,
            avg_blob_us,
            (avg_blob_us as f64) / 1000.0,
            (avg_blob_us as f64) / (avg_vec_us as f64).max(1.0)
        );
    }

    println!("\n=======================================================\n");
}
