use uuid::Uuid;

pub const SCHEMA: &[u8] = b"meta/schema";
pub const RECORDS: &[u8] = b"record/";
pub const PROJECTIONS: &[u8] = b"projection/";

pub fn record(id: Uuid) -> Vec<u8> {
    format!("record/{id}").into_bytes()
}
pub fn request(id: Uuid) -> Vec<u8> {
    format!("request/{id}").into_bytes()
}
pub fn projection(id: Uuid) -> Vec<u8> {
    format!("projection/{id}").into_bytes()
}
pub fn members(list: Uuid) -> Vec<u8> {
    format!("member/{list}/").into_bytes()
}
pub fn member(list: Uuid, item: Uuid) -> Vec<u8> {
    format!("member/{list}/{item}").into_bytes()
}
