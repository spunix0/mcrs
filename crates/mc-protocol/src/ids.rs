//! Packet IDs for protocol 777 (Minecraft 26.3), taken from the server's `--reports` packets.json.

pub mod handshake {
    pub const INTENTION: i32 = 0;
}

pub mod status {
    pub mod c2s {
        pub const STATUS_REQUEST: i32 = 0;
        pub const PING_REQUEST: i32 = 1;
    }
    pub mod s2c {
        pub const STATUS_RESPONSE: i32 = 0;
        pub const PONG_RESPONSE: i32 = 1;
    }
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
        pub const CLIENT_INFORMATION: i32 = 0;
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
        pub const ATTACK: i32 = 1;
        pub const CHAT_COMMAND: i32 = 7;
        pub const CHAT: i32 = 9;
        pub const CHUNK_BATCH_RECEIVED: i32 = 11;
        pub const CONTAINER_CLOSE: i32 = 19;
        pub const CLIENT_COMMAND: i32 = 12;
        pub const CLIENT_TICK_END: i32 = 13;
        pub const CONFIGURATION_ACKNOWLEDGED: i32 = 16;
        pub const KEEP_ALIVE: i32 = 28;
        pub const MOVE_PLAYER_POS_ROT: i32 = 31;
        pub const PLAYER_ACTION: i32 = 41;
        pub const PLAYER_LOADED: i32 = 44;
        pub const PONG: i32 = 45;
        pub const SET_CARRIED_ITEM: i32 = 54;
        pub const USE_ITEM_ON: i32 = 66;
        pub const USE_ITEM: i32 = 67;
        pub const CONTAINER_CLICK: i32 = 18;
        pub const PLAYER_COMMAND: i32 = 42;
        pub const PUNCH: i32 = 46;
        pub const CLIENT_INFORMATION: i32 = 14;
        pub const PICK_ITEM_FROM_BLOCK: i32 = 36;
        pub const SET_CREATIVE_MODE_SLOT: i32 = 57;
    }
    pub mod s2c {
        pub const ADD_ENTITY: i32 = 1;
        pub const ENTITY_POSITION_SYNC: i32 = 35;
        pub const MOVE_ENTITY_POS: i32 = 54;
        pub const MOVE_ENTITY_POS_ROT: i32 = 55;
        pub const MOVE_ENTITY_ROT: i32 = 57;
        pub const REMOVE_ENTITIES: i32 = 78;
        pub const ROTATE_HEAD: i32 = 85;
        pub const TELEPORT_ENTITY: i32 = 128;
        pub const BLOCK_UPDATE: i32 = 8;
        pub const CONTAINER_SET_CONTENT: i32 = 18;
        pub const CONTAINER_SET_SLOT: i32 = 20;
        pub const SET_HELD_SLOT: i32 = 107;
        pub const SET_PLAYER_INVENTORY: i32 = 110;
        pub const LIGHT_UPDATE: i32 = 49;
        pub const LEVEL_EVENT: i32 = 47;
        pub const SOUND_ENTITY: i32 = 118;
        pub const SOUND: i32 = 119;
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
        pub const PLAYER_CHAT: i32 = 66;
        pub const DISGUISED_CHAT: i32 = 33;
        pub const ANIMATE: i32 = 2;
        pub const BLOCK_DESTRUCTION: i32 = 5;
        pub const CHANGE_DIFFICULTY: i32 = 10;
        pub const CLEAR_TITLES: i32 = 14;
        pub const CONTAINER_CLOSE: i32 = 17;
        pub const CONTAINER_SET_DATA: i32 = 19;
        pub const DAMAGE_EVENT: i32 = 25;
        pub const ENTITY_EVENT: i32 = 34;
        pub const HURT_ANIMATION: i32 = 43;
        pub const OPEN_SCREEN: i32 = 60;
        pub const PLAYER_ABILITIES: i32 = 65;
        pub const PLAYER_COMBAT_KILL: i32 = 69;
        pub const PLAYER_INFO_REMOVE: i32 = 70;
        pub const PLAYER_INFO_UPDATE: i32 = 71;
        pub const RESPAWN: i32 = 84;
        pub const SET_ACTION_BAR_TEXT: i32 = 89;
        pub const SET_CURSOR_ITEM: i32 = 98;
        pub const SET_ENTITY_DATA: i32 = 101;
        pub const SET_ENTITY_MOTION: i32 = 103;
        pub const SET_EXPERIENCE: i32 = 105;
        pub const SET_SUBTITLE_TEXT: i32 = 114;
        pub const SET_TIME: i32 = 115;
        pub const SET_TITLE_TEXT: i32 = 116;
        pub const SET_TITLES_ANIMATION: i32 = 117;
        pub const TAB_LIST: i32 = 125;
        pub const TAKE_ITEM_ENTITY: i32 = 127;
    }
}
