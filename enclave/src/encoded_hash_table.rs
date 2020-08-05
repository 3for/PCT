use std::vec::Vec;
use std::collections::HashSet;
use std::mem;
use primitive::*;
use constant::*;
use mapped_encoded_query_buffer::MappedEncodedQueryBuffer;
use encoded_result_buffer::EncodedResultBuffer;

#[derive(Clone, Default, Debug)]
pub struct EncodedHashTable {
    pub map: HashSet<EncodedValue>,
}
impl EncodedHashTable {
    pub fn new() -> Self {
        EncodedHashTable {
            map: HashSet::with_capacity(THREASHOLD)
        }
    }

    pub fn intersect(&self, mapped_query_buffer: &MappedEncodedQueryBuffer, result: &mut EncodedResultBuffer) {
        for encoded_value_vec in mapped_query_buffer.map.iter() {
            if self.map.contains(encoded_value_vec) {
                result.data.push(*encoded_value_vec);
            }
        }
    }

    pub fn build_dictionary_buffer(
        &mut self,
        encoded_value_vec: &Vec<u8>,
        size: usize,
    ) {
        for i in 0usize..(size) {
            let mut encoded_value: EncodedValue = [0u8; ENCODEDVALUE_SIZE];
            encoded_value.copy_from_slice(&encoded_value_vec[ENCODEDVALUE_SIZE*i..ENCODEDVALUE_SIZE*(i+1)]);
            self.map.insert(encoded_value);
        }
    }

    pub fn calc_memory(&self) {
        println!("[HashTable] Q size = {} bytes", (self.map.capacity() * 11 / 10) * (mem::size_of::<EncodedValue>() + mem::size_of::<()>() + mem::size_of::<u64>()));
    }
}