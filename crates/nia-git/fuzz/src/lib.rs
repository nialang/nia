#![allow(dead_code)]

pub fn packet_lines(input: &[u8]) {
    nia_git::fuzzing::packet_lines(input);
}

pub fn loose_object(input: &[u8]) {
    nia_git::fuzzing::loose_object(input);
}

pub fn delta(input: &[u8]) {
    nia_git::fuzzing::delta(input);
}

pub fn pack_object(input: &[u8]) {
    nia_git::fuzzing::pack_object(input);
}

pub fn http_response(input: &[u8]) {
    nia_git::fuzzing::http_response(input);
}

pub fn ssh_agent_identities(input: &[u8]) {
    nia_git::fuzzing::ssh_agent_identities(input);
}

pub fn ssh_kexinit(input: &[u8]) {
    nia_git::fuzzing::ssh_kexinit(input);
}

pub fn openssh_identity(input: &[u8]) {
    nia_git::fuzzing::openssh_identity(input);
}

pub fn known_hosts(input: &[u8]) {
    nia_git::fuzzing::known_hosts(input);
}
