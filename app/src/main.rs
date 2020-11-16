// Licensed to the Apache Software Foundation (ASF) under one
// or more contributor license agreements.  See the NOTICE file
// distributed with this work for additional information
// regarding copyright ownership.  The ASF licenses this file
// to you under the Apache License, Version 2.0 (the
// "License"); you may not use this file except in compliance
// with the License.  You may obtain a copy of the License at
//
//   http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing,
// software distributed under the License is distributed on an
// "AS IS" BASIS, WITHOUT WARRANTIES OR CONDITIONS OF ANY
// KIND, either express or implied.  See the License for the
// specific language governing permissions and limitations
// under the License..

extern crate sgx_types;
extern crate sgx_urts;
extern crate serde;
extern crate serde_json;
extern crate fst;
extern crate bincode;
extern crate hex;

use std::env;

use sgx_types::*;

mod query_data;
use query_data::*;

// ecallsはnamedで呼び出す
mod ecalls;
use ecalls::{ 
    upload_encoded_query_data, 
    init_enclave,
    private_encode_contact_trace, get_encoded_result
};

mod central_data;
use central_data::*;

mod util;
use util::*;

pub const QUERY_ID_SIZE_U8: usize = 8;
pub const QUERY_RESULT_U8: usize = 1;
pub const RESPONSE_DATA_SIZE_U8: usize = QUERY_ID_SIZE_U8 + QUERY_RESULT_U8;

/*
    args[0] = threashold of each chunk block size
    args[1] = query data file path
    args[2] = central data file path
*/
fn _get_options() -> Vec<String> {
    let args: Vec<String> = env::args().skip(1).collect();
    if args.len() != 3 {
        println!(" ERROR bin/app needs 4 arguments!");
        println!("    args[0] = threashold of each chunk block size");
        println!("    args[1] = query data file path");
        println!("    args[2] = central data file path");
        std::process::exit(-1);
    }
    args
}

fn main() {
    #[cfg(feature = "hashtable")]
    encodedHasing();
    #[cfg(feature = "fsa")]
    finiteStateTranducer();
}

fn encodedHasing() {
    let args = _get_options();
    /* parameters */
    let threashould: usize = args[0].parse().unwrap();
    let q_filename = &args[1];
    let c_filename = &args[2];

    let mut clocker = Clocker::new();

    /* read central data */
    clocker.set_and_start("Read Central Data");
    let external_data = EncodedData::read_raw_from_file(c_filename);
    clocker.stop("Read Central Data");
    let central_data_size = external_data.size();

    /* preprocess central data */
    clocker.set_and_start("Distribute central data");
    let mut R: CentralHashSet = CentralHashSet::from_EncodedData(external_data, threashould);
    clocker.stop("Distribute central data");

    /* initialize enclave */
    println!("init_enclave...");
    clocker.set_and_start("ECALL init_enclave");
    let enclave = match init_enclave() {
        Ok(r) => {
            // println!(" Init Enclave Successful {}!", r.geteid());
            r
        },
        Err(x) => {
            println!(" Init Enclave Failed {}!", x.as_str());
            return;
        },
    };
    clocker.stop("ECALL init_enclave");

    /* read query data */
    clocker.set_and_start("Read Query Data");
    let query_data = EncodedQueryData::read_raw_from_file(q_filename);
    clocker.stop("Read Query Data");

    /* encrypt and upload query data */
    let total_data_vec = query_data.total_data_to_u8();
    clocker.set_and_start("ECALL upload_query_data");
    let mut retval = sgx_status_t::SGX_SUCCESS;
    let result = unsafe {
        upload_encoded_query_data(
            enclave.geteid(),
            &mut retval,
            total_data_vec.as_ptr() as * const u8,
            total_data_vec.len(),
            query_data.client_size,
            query_data.query_id_list().as_ptr() as * const u64
        )
    };
    match result {
        sgx_status_t::SGX_SUCCESS => {
            // println!("[UNTRUSTED] upload_query_data Succes!");
        },
        _ => {
            println!("[UNTRUSTED] upload_query_data Failed {}!", result.as_str());
            return;
        }
    }
    clocker.stop("ECALL upload_query_data");

    /* main logic contact tracing */
    let mut chunk_index: usize = 0;
    let last = R.len() - 1;
    clocker.set_and_start("ECALL private_contact_trace");
    while last >= chunk_index {
        let chunk: &Vec<u8> = R.prepare_sgx_data(chunk_index);
        let result = unsafe {
            private_encode_contact_trace(
                enclave.geteid(),
                &mut retval,
                chunk.as_ptr() as * const u8,
                chunk.len(),
            )
        };
        match result {
            sgx_status_t::SGX_SUCCESS => {
                print!("\r[UNTRUSTED] private_contact_trace Succes! {} th iteration", chunk_index);
            },
            _ => {
                println!("[UNTRUSTED] private_contact_trace Failed {}!", result.as_str());
                return;
            }
        }
        chunk_index += 1;
    }
    println!("");
    
    clocker.stop("ECALL private_contact_trace");

    /* response reconstruction */
    clocker.set_and_start("ECALL get_result");
    let response_size = query_data.client_size * RESPONSE_DATA_SIZE_U8;
    let mut response: Vec<u8> = vec![0; response_size];
    let result = unsafe {
        get_encoded_result(
            enclave.geteid(),
            &mut retval,
            response.as_mut_ptr(),
            response_size
        )
    };
    match result {
        sgx_status_t::SGX_SUCCESS => {
            // println!("[UNTRUSTED] get_result Succes!");
        },
        _ => {
            println!("[UNTRUSTED] get_result Failed {}!", result.as_str());
            return;
        }
    }
    clocker.stop("ECALL get_result");
    
    let mut positive_queries = vec![];
    for i in 0..query_data.client_size {
        /* decryption for each clients using their keys */ 
        let query_id: QueryId = query_id_from_u8(&response[i*RESPONSE_DATA_SIZE_U8..i*RESPONSE_DATA_SIZE_U8+QUERY_ID_SIZE_U8]);
        let mut shared_key: [u8; 16] = [0; 16];
        shared_key[..8].copy_from_slice(&query_id.to_be_bytes());
        let counter_block: [u8; 16] = COUNTER_BLOCK;
        let ctr_inc_bits: u32 = SGXSSL_CTR_BITS;
        let src_len: usize = QUERY_RESULT_U8;
        let mut result: Vec<u8> = vec![0; src_len];
        let ret = unsafe {
            util::sgx_aes_ctr_decrypt(
                &shared_key,
                response[i*RESPONSE_DATA_SIZE_U8+QUERY_ID_SIZE_U8..i*RESPONSE_DATA_SIZE_U8+RESPONSE_DATA_SIZE_U8].as_ptr() as *const u8,
                src_len as u32,
                &counter_block as * const u8,
                ctr_inc_bits,
                result.as_mut_ptr()
            )
        };
        if ret < 0 { println!("Error in CTR decryption."); std::process::exit(-1); }
        if result[0] > 0 {
            positive_queries.push(query_id);
        }
    }
    println!("positive result queryIds: {:?}", positive_queries);

    /* finish */
    enclave.destroy();
    // println!("[UNTRUSTED] All process is successful!!");
    clocker.show_all();
    let now: String = get_timestamp();

    #[cfg(feature = "th48")]
    let method = "th48";
    #[cfg(feature = "gp10")]
    let method = "gp10";

    write_to_file(
        format!("data/result/result-{}.txt", now),
        "hash table".to_string(),
        method.to_string(),
        c_filename.to_string(),
        central_data_size,
        q_filename.to_string(),
        query_data.client_size,
        1440,
        threashould,
        clocker
    );
    
}

fn finiteStateTranducer() {
    let args = _get_options();
    /* parameters */
    let threashould: usize = args[0].parse().unwrap();
    let q_filename = &args[1];
    let c_filename = &args[2];

    let mut clocker = Clocker::new();

    /* read central data */
    clocker.set_and_start("Read Central Data");
    let external_data = EncodedData::read_raw_from_file(c_filename);
    clocker.stop("Read Central Data");
    let central_data_size = external_data.size();

    /* preprocess central data */
    clocker.set_and_start("Distribute central data");
    let mut R: CentralFST = CentralFST::from_EncodedData(external_data, threashould);
    clocker.stop("Distribute central data");

    /* initialize enclave */
    println!("init_enclave...");
    clocker.set_and_start("ECALL init_enclave");
    let enclave = match init_enclave() {
        Ok(r) => {
            // println!("[UNTRUSTED] Init Enclave Successful {}!", r.geteid());
            r
        },
        Err(x) => {
            println!("[UNTRUSTED] Init Enclave Failed {}!", x.as_str());
            return;
        },
    };
    clocker.stop("ECALL init_enclave");

    /* read query data */
    clocker.set_and_start("Read Query Data");
    let query_data = EncodedQueryData::read_raw_from_file(q_filename);
    clocker.stop("Read Query Data");

    /* encrypt and upload query data */
    let total_data_vec = query_data.total_data_to_u8();
    clocker.set_and_start("ECALL upload_query_data");
    let mut retval = sgx_status_t::SGX_SUCCESS;
    let result = unsafe {
        upload_encoded_query_data(
            enclave.geteid(),
            &mut retval,
            total_data_vec.as_ptr() as * const u8,
            total_data_vec.len(),
            query_data.client_size,
            query_data.query_id_list().as_ptr() as * const u64
        )
    };
    match result {
        sgx_status_t::SGX_SUCCESS => {
            // println!("[UNTRUSTED] upload_query_data Succes!");
        },
        _ => {
            println!("[UNTRUSTED] upload_query_data Failed {}!", result.as_str());
            return;
        }
    }
    clocker.stop("ECALL upload_query_data");

    /* main logic contact tracing */
    let mut chunk_index: usize = 0;
    let last = R.len() - 1;
    clocker.set_and_start("ECALL private_contact_trace");
    while last >= chunk_index {
        let chunk: &Vec<u8> = R.prepare_sgx_data(chunk_index);
        let result = unsafe {
            private_encode_contact_trace(
                enclave.geteid(),
                &mut retval,
                chunk.as_ptr() as * const u8,
                chunk.len()
            )
        };
        match result {
            sgx_status_t::SGX_SUCCESS => {
                print!("\r[UNTRUSTED] private_contact_trace Succes! {} th iteration", chunk_index);
            },
            _ => {
                println!("[UNTRUSTED] private_contact_trace Failed {}!", result.as_str());
                return;
            }
        }
        chunk_index += 1;
    }
    println!("");
    
    clocker.stop("ECALL private_contact_trace");

    /* response reconstruction */
    clocker.set_and_start("ECALL get_result");
    let response_size = query_data.client_size * RESPONSE_DATA_SIZE_U8;
    let mut response: Vec<u8> = vec![0; response_size];
    let result = unsafe {
        get_encoded_result(
            enclave.geteid(),
            &mut retval,
            response.as_mut_ptr(),
            response_size
        )
    };
    match result {
        sgx_status_t::SGX_SUCCESS => {
            // println!("[UNTRUSTED] get_result Succes!");
        },
        _ => {
            println!("[UNTRUSTED] get_result Failed {}!", result.as_str());
            return;
        }
    }
    clocker.stop("ECALL get_result");
    
    let mut positive_queries = vec![];
    for i in 0..query_data.client_size {
        /* decryption for each clients using their keys */ 
        let query_id: QueryId = query_id_from_u8(&response[i*RESPONSE_DATA_SIZE_U8..i*RESPONSE_DATA_SIZE_U8+QUERY_ID_SIZE_U8]);
        let mut shared_key: [u8; 16] = [0; 16];
        shared_key[..8].copy_from_slice(&query_id.to_be_bytes());
        let counter_block: [u8; 16] = COUNTER_BLOCK;
        let ctr_inc_bits: u32 = SGXSSL_CTR_BITS;
        let src_len: usize = QUERY_RESULT_U8;
        let mut result: Vec<u8> = vec![0; src_len];
        let ret = unsafe {
            util::sgx_aes_ctr_decrypt(
                &shared_key,
                response[i*RESPONSE_DATA_SIZE_U8+QUERY_ID_SIZE_U8..i*RESPONSE_DATA_SIZE_U8+RESPONSE_DATA_SIZE_U8].as_ptr() as *const u8,
                src_len as u32,
                &counter_block as * const u8,
                ctr_inc_bits,
                result.as_mut_ptr()
            )
        };
        if ret < 0 { println!("Error in CTR decryption."); std::process::exit(-1); }
        if result[0] > 0 {
            positive_queries.push(query_id);
        }
    }
    println!("positive result queryIds: {:?}", positive_queries);

    /* finish */
    enclave.destroy();
    // println!("[UNTRUSTED] All process is successful!!");
    clocker.show_all();
    let now: String = get_timestamp();
    
    #[cfg(feature = "th48")]
    let method = "th48";
    #[cfg(feature = "gp10")]
    let method = "gp10";

    write_to_file(
        format!("data/result/result-{}.txt", now),
        "fst".to_string(),
        method.to_string(),
        c_filename.to_string(),
        central_data_size,
        q_filename.to_string(),
        query_data.client_size,
        1440,
        threashould,
        clocker
    );
}