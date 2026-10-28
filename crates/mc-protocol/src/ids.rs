//! Packet IDs for protocol 777 (Minecraft 26.3), taken from the server's `--reports` packets.json.

pub mod handshake {
    pub const INTENTION: i32 = 0;
}

pub mod login {
    pub mod c2s {
        pub const HELLO: i32 = 0;
        pub const CUSTOM_QUERY_ANSWER: i32 = 2;
        pub const LOGIN_ACKNOWLEDGED: i32 = 3;
    }
    pub mod s2c {
        pub const DISCONNECT: i32 = 0;
        pub const HELLO: i32 = 1;
        pub const LOGIN_FINISHED: i32 = 2;
        pub const COMPRESSION: i32 = 3;
        pub const CUSTOM_QUERY: i32 = 4;
    }
}

pub mod config {
    pub mod c2s {
        pub const FINISH_CONFIGURATION: i32 = 3;
        pub const KEEP_ALIVE: i32 = 4;
        pub const PONG: i32 = 5;
        pub const SELECT_KNOWN_PACKS: i32 = 7;
        pub const ACCEPT_CODE_OF_CONDUCT: i32 = 9;
    }
    pub mod s2c {
        pub const DISCONNECT: i32 = 2;
        pub const FINISH_CONFIGURATION: i32 = 3;
        pub const KEEP_ALIVE: i32 = 4;
        pub const PING: i32 = 5;
        pub const REGISTRY_DATA: i32 = 7;
        pub const SELECT_KNOWN_PACKS: i32 = 15;
        pub const CODE_OF_CONDUCT: i32 = 20;
    }
}

pub mod play {
    pub mod c2s {
        pub const ACCEPT_TELEPORTATION: i32 = 0;
        pub const CHAT_COMMAND: i32 = 7;
        pub const CHAT: i32 = 9;
        pub const CHUNK_BATCH_RECEIVED: i32 = 11;
        pub const CLIENT_COMMAND: i32 = 12;
        pub const CLIENT_TICK_END: i32 = 13;
        pub const CONFIGURATION_ACKNOWLEDGED: i32 = 16;
        pub const KEEP_ALIVE: i32 = 28;
        pub const MOVE_PLAYER_POS_ROT: i32 = 31;
        pub const PLAYER_LOADED: i32 = 44;
        pub const PONG: i32 = 45;
    }
    pub mod s2c {
        pub const BLOCK_UPDATE: i32 = 8;
        pub const LIGHT_UPDATE: i32 = 49;
        pub const SECTION_BLOCKS_UPDATE: i32 = 86;
        pub const CHUNK_BATCH_FINISHED: i32 = 11;
        pub const CHUNK_BATCH_START: i32 = 12;
        pub const DISCONNECT: i32 = 32;
        pub const FORGET_LEVEL_CHUNK: i32 = 38;
        pub const GAME_EVENT: i32 = 39;
        pub const KEEP_ALIVE: i32 = 45;
        pub const LEVEL_CHUNK_WITH_LIGHT: i32 = 46;
        pub const LOGIN: i32 = 50;
        pub const PING: i32 = 62;
        pub const PLAYER_POSITION: i32 = 73;
        pub const SET_HEALTH: i32 = 106;
        pub const START_CONFIGURATION: i32 = 120;
        pub const SYSTEM_CHAT: i32 = 124;
    }
}
