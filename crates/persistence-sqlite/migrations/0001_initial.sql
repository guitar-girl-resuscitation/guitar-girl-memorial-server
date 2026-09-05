PRAGMA foreign_keys=ON;

CREATE TABLE schema_migrations(version INTEGER PRIMARY KEY, applied_at INTEGER NOT NULL) STRICT;
INSERT INTO schema_migrations VALUES(4, unixepoch());

CREATE TABLE save_slots(
  usn INTEGER PRIMARY KEY CHECK(usn>0), user_id TEXT NOT NULL UNIQUE
    CHECK(length(user_id)>0 AND user_id NOT GLOB '*[^0-9]*'),
  display_name TEXT NOT NULL, created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL
) STRICT;
CREATE TRIGGER immutable_slot_usn BEFORE UPDATE OF usn ON save_slots BEGIN SELECT RAISE(ABORT,'USN is immutable'); END;
CREATE TRIGGER immutable_slot_uid BEFORE UPDATE OF user_id ON save_slots BEGIN SELECT RAISE(ABORT,'U_id is immutable'); END;

CREATE TABLE device_bindings(
  device_key TEXT PRIMARY KEY, usn INTEGER NOT NULL REFERENCES save_slots(usn) ON DELETE CASCADE, created_at INTEGER NOT NULL
) STRICT;
CREATE TABLE runtime_selection(
  singleton INTEGER PRIMARY KEY CHECK(singleton=1), active_usn INTEGER REFERENCES save_slots(usn),
  pending_usn INTEGER REFERENCES save_slots(usn), session_nonce INTEGER NOT NULL DEFAULT 0
) STRICT;
INSERT INTO runtime_selection(singleton) VALUES(1);
CREATE TABLE login_sessions(
  nonce INTEGER PRIMARY KEY, usn INTEGER NOT NULL REFERENCES save_slots(usn) ON DELETE CASCADE, capability_hash BLOB NOT NULL UNIQUE,
  started_at INTEGER NOT NULL, device_time INTEGER NOT NULL, utc_offset_minutes INTEGER NOT NULL,
  last_request_seq INTEGER NOT NULL DEFAULT 0, ended_at INTEGER
) STRICT;

CREATE TABLE user_core(
  usn INTEGER PRIMARY KEY REFERENCES save_slots(usn) ON DELETE CASCADE, nickname TEXT NOT NULL DEFAULT 'Guitar Girl',
  fans REAL NOT NULL DEFAULT 0 CHECK(fans>=0), fan_level INTEGER NOT NULL DEFAULT 1 CHECK(fan_level>0),
  avatar_id INTEGER DEFAULT 1, title_id INTEGER DEFAULT 1, last_save_time INTEGER NOT NULL DEFAULT 0,
  device_uuid TEXT NOT NULL DEFAULT 'reborn-local-device'
) STRICT;
CREATE TABLE area_state(
  usn INTEGER NOT NULL REFERENCES save_slots(usn) ON DELETE CASCADE, area INTEGER NOT NULL,
  area_level INTEGER NOT NULL DEFAULT 1, current_costume INTEGER, current_guitar INTEGER, current_music INTEGER,
  fans REAL NOT NULL DEFAULT 0 CHECK(fans>=0), candy REAL NOT NULL DEFAULT 0 CHECK(candy>=0),
  gp REAL NOT NULL DEFAULT 0, unlocked INTEGER NOT NULL DEFAULT 0 CHECK(unlocked IN(0,1)),
  PRIMARY KEY(usn,area)
) STRICT, WITHOUT ROWID;
CREATE TABLE currencies(
  usn INTEGER NOT NULL REFERENCES save_slots(usn) ON DELETE CASCADE, kind TEXT NOT NULL,
  amount REAL NOT NULL DEFAULT 0 CHECK(amount>=0), PRIMARY KEY(usn,kind)
) STRICT, WITHOUT ROWID;

CREATE TABLE owned_content(
  usn INTEGER NOT NULL REFERENCES save_slots(usn) ON DELETE CASCADE, kind TEXT NOT NULL, area INTEGER NOT NULL DEFAULT 0,
  content_id INTEGER NOT NULL, acquired_at INTEGER NOT NULL, source TEXT NOT NULL,
  -- Content ids are globally unique within a kind in the shipped v8 master
  -- data. Keeping area in the identity allowed a malformed client save to
  -- create duplicate UserMusic/UserCostume rows with the same I_id, which
  -- breaks the client's dictionaries during login projection.
  PRIMARY KEY(usn,kind,content_id)
) STRICT, WITHOUT ROWID;
CREATE TABLE equipped_content(
  usn INTEGER NOT NULL REFERENCES save_slots(usn) ON DELETE CASCADE, kind TEXT NOT NULL, area INTEGER NOT NULL,
  content_id INTEGER NOT NULL, updated_at INTEGER NOT NULL, PRIMARY KEY(usn,kind,area)
) STRICT, WITHOUT ROWID;
CREATE TABLE content_levels(
  usn INTEGER NOT NULL REFERENCES save_slots(usn) ON DELETE CASCADE, kind TEXT NOT NULL, content_id INTEGER NOT NULL,
  level INTEGER NOT NULL DEFAULT 1, reward_level INTEGER NOT NULL DEFAULT 0,
  PRIMARY KEY(usn,kind,content_id)
) STRICT, WITHOUT ROWID;
CREATE TABLE music_state(
  usn INTEGER NOT NULL REFERENCES save_slots(usn) ON DELETE CASCADE, music_id INTEGER NOT NULL,
  encore_appears INTEGER NOT NULL DEFAULT 0, encore_follower_id INTEGER,
  encore_ready_at INTEGER NOT NULL DEFAULT 0, ch3_active_until INTEGER NOT NULL DEFAULT 0,
  PRIMARY KEY(usn,music_id)
) STRICT, WITHOUT ROWID;
CREATE TABLE skill_activation(
  usn INTEGER NOT NULL REFERENCES save_slots(usn) ON DELETE CASCADE, skill_id INTEGER NOT NULL,
  active INTEGER NOT NULL DEFAULT 0 CHECK(active IN(0,1)), active_from INTEGER NOT NULL DEFAULT 0,
  active_until INTEGER NOT NULL DEFAULT 0, reset_ready_at INTEGER NOT NULL DEFAULT 0,
  PRIMARY KEY(usn,skill_id)
) STRICT, WITHOUT ROWID;
CREATE TABLE buff_timers(
  usn INTEGER NOT NULL REFERENCES save_slots(usn) ON DELETE CASCADE, buff_id INTEGER NOT NULL,
  active_at INTEGER NOT NULL, expires_at INTEGER NOT NULL CHECK(expires_at>=active_at),
  PRIMARY KEY(usn,buff_id)
) STRICT, WITHOUT ROWID;
CREATE TABLE ad_levels(usn INTEGER NOT NULL REFERENCES save_slots(usn) ON DELETE CASCADE, ad_level_id INTEGER NOT NULL, level INTEGER NOT NULL DEFAULT 1 CHECK(level>0), experience INTEGER NOT NULL DEFAULT 0 CHECK(experience>=0), PRIMARY KEY(usn,ad_level_id)) STRICT, WITHOUT ROWID;
CREATE TABLE ad_state(usn INTEGER NOT NULL REFERENCES save_slots(usn) ON DELETE CASCADE, ad_id INTEGER NOT NULL, local_day INTEGER NOT NULL, day_count INTEGER NOT NULL DEFAULT 0, total_count INTEGER NOT NULL DEFAULT 0, last_view_time INTEGER NOT NULL DEFAULT 0, PRIMARY KEY(usn,ad_id)) STRICT, WITHOUT ROWID;

CREATE TABLE tutorial_completed(usn INTEGER NOT NULL REFERENCES save_slots(usn) ON DELETE CASCADE, tutorial_id INTEGER NOT NULL, completed_at INTEGER NOT NULL, PRIMARY KEY(usn,tutorial_id)) STRICT, WITHOUT ROWID;
CREATE TABLE one_time_flags(usn INTEGER NOT NULL REFERENCES save_slots(usn) ON DELETE CASCADE, flag TEXT NOT NULL, value INTEGER NOT NULL, updated_at INTEGER NOT NULL, PRIMARY KEY(usn,flag)) STRICT, WITHOUT ROWID;
CREATE TABLE achievement_progress(usn INTEGER NOT NULL REFERENCES save_slots(usn) ON DELETE CASCADE, achievement_id INTEGER NOT NULL, quantity REAL NOT NULL DEFAULT 0, quantity_text TEXT NOT NULL DEFAULT '0', updated_at INTEGER NOT NULL, PRIMARY KEY(usn,achievement_id)) STRICT, WITHOUT ROWID;
CREATE TABLE achievement_claims(usn INTEGER NOT NULL REFERENCES save_slots(usn) ON DELETE CASCADE, achievement_id INTEGER NOT NULL, level INTEGER NOT NULL, claimed_at INTEGER NOT NULL, PRIMARY KEY(usn,achievement_id,level)) STRICT, WITHOUT ROWID;
CREATE TABLE daily_missions(usn INTEGER NOT NULL REFERENCES save_slots(usn) ON DELETE CASCADE, mission_id INTEGER NOT NULL, quantity REAL NOT NULL DEFAULT 0, updated_at INTEGER NOT NULL, PRIMARY KEY(usn,mission_id)) STRICT, WITHOUT ROWID;
CREATE TABLE daily_mission_claims(usn INTEGER NOT NULL REFERENCES save_slots(usn) ON DELETE CASCADE, local_day INTEGER NOT NULL, mission_id INTEGER NOT NULL, level INTEGER NOT NULL, claimed_at INTEGER NOT NULL, PRIMARY KEY(usn,local_day,mission_id,level)) STRICT, WITHOUT ROWID;
CREATE TABLE attendance_days(usn INTEGER NOT NULL REFERENCES save_slots(usn) ON DELETE CASCADE, local_day INTEGER NOT NULL, claimed_at INTEGER NOT NULL, PRIMARY KEY(usn,local_day)) STRICT, WITHOUT ROWID;
CREATE TABLE attendance_state(usn INTEGER PRIMARY KEY REFERENCES save_slots(usn) ON DELETE CASCADE, attendance_count INTEGER NOT NULL DEFAULT 0, continuous_count INTEGER NOT NULL DEFAULT 0, max_continuous_count INTEGER NOT NULL DEFAULT 0, last_local_day INTEGER) STRICT;
CREATE TABLE event_points(usn INTEGER NOT NULL REFERENCES save_slots(usn) ON DELETE CASCADE, event_type TEXT NOT NULL, data_id INTEGER NOT NULL, points INTEGER NOT NULL DEFAULT 0, step INTEGER NOT NULL DEFAULT 0, version INTEGER NOT NULL DEFAULT 1, PRIMARY KEY(usn,event_type,data_id,version)) STRICT, WITHOUT ROWID;
CREATE TABLE selected_rewards(usn INTEGER NOT NULL REFERENCES save_slots(usn) ON DELETE CASCADE, group_id INTEGER NOT NULL, selection_id INTEGER NOT NULL, reward_group_id INTEGER NOT NULL, alternate_reward_group_id INTEGER NOT NULL, selected_at INTEGER NOT NULL, PRIMARY KEY(usn,group_id)) STRICT, WITHOUT ROWID;

CREATE TABLE followers(usn INTEGER NOT NULL REFERENCES save_slots(usn) ON DELETE CASCADE, follower_id INTEGER NOT NULL, level INTEGER NOT NULL DEFAULT 0, reward_level INTEGER NOT NULL DEFAULT 0, unlocked_at INTEGER, PRIMARY KEY(usn,follower_id)) STRICT, WITHOUT ROWID;
CREATE TABLE follower_quests(usn INTEGER NOT NULL REFERENCES save_slots(usn) ON DELETE CASCADE, chain_id INTEGER NOT NULL, stage_id INTEGER NOT NULL, completed INTEGER NOT NULL DEFAULT 0 CHECK(completed IN(0,1)), infinite INTEGER NOT NULL DEFAULT 0 CHECK(infinite IN(0,1)), updated_at INTEGER NOT NULL, PRIMARY KEY(usn,chain_id,stage_id)) STRICT, WITHOUT ROWID;
CREATE TABLE quest_conditions(usn INTEGER NOT NULL REFERENCES save_slots(usn) ON DELETE CASCADE, chain_id INTEGER NOT NULL, stage_id INTEGER NOT NULL, condition_id INTEGER NOT NULL, quantity REAL NOT NULL DEFAULT 0, claimed INTEGER NOT NULL DEFAULT 0 CHECK(claimed IN(0,1)), PRIMARY KEY(usn,chain_id,stage_id,condition_id)) STRICT, WITHOUT ROWID;
CREATE TABLE gifts(usn INTEGER NOT NULL REFERENCES save_slots(usn) ON DELETE CASCADE, gift_id INTEGER NOT NULL, quantity INTEGER NOT NULL DEFAULT 0 CHECK(quantity>=0), PRIMARY KEY(usn,gift_id)) STRICT, WITHOUT ROWID;
CREATE TABLE affection(usn INTEGER NOT NULL REFERENCES save_slots(usn) ON DELETE CASCADE, follower_id INTEGER NOT NULL, experience INTEGER NOT NULL DEFAULT 0 CHECK(experience>=0), level INTEGER NOT NULL DEFAULT 1, PRIMARY KEY(usn,follower_id)) STRICT, WITHOUT ROWID;
CREATE TABLE affection_claims(usn INTEGER NOT NULL REFERENCES save_slots(usn) ON DELETE CASCADE, follower_id INTEGER NOT NULL, reward_level INTEGER NOT NULL, claimed_at INTEGER NOT NULL, PRIMARY KEY(usn,follower_id,reward_level)) STRICT, WITHOUT ROWID;

CREATE TABLE messenger_rooms(usn INTEGER NOT NULL REFERENCES save_slots(usn) ON DELETE CASCADE, room_id INTEGER NOT NULL, state INTEGER NOT NULL DEFAULT 0, last_confirm_index INTEGER NOT NULL DEFAULT 0, unlock_group_list TEXT NOT NULL DEFAULT '', update_time_ticks INTEGER NOT NULL DEFAULT 0, updated_at INTEGER NOT NULL, PRIMARY KEY(usn,room_id)) STRICT, WITHOUT ROWID;
CREATE TABLE messenger_progress(usn INTEGER NOT NULL REFERENCES save_slots(usn) ON DELETE CASCADE, room_id INTEGER NOT NULL, message_id INTEGER NOT NULL, choice_id INTEGER, completed INTEGER NOT NULL DEFAULT 0 CHECK(completed IN(0,1)), PRIMARY KEY(usn,room_id,message_id)) STRICT, WITHOUT ROWID;
CREATE TABLE gallery_unlocks(usn INTEGER NOT NULL REFERENCES save_slots(usn) ON DELETE CASCADE, album_id INTEGER NOT NULL, unlocked_at INTEGER NOT NULL, PRIMARY KEY(usn,album_id)) STRICT, WITHOUT ROWID;
CREATE TABLE music_reviews(usn INTEGER NOT NULL REFERENCES save_slots(usn) ON DELETE CASCADE, music_id INTEGER NOT NULL, difficulty INTEGER NOT NULL, point REAL NOT NULL, updated_at INTEGER NOT NULL, PRIMARY KEY(usn,music_id,difficulty)) STRICT, WITHOUT ROWID;
CREATE TABLE music_bookmarks(usn INTEGER NOT NULL REFERENCES save_slots(usn) ON DELETE CASCADE, music_id INTEGER NOT NULL, flag INTEGER NOT NULL CHECK(flag IN(0,1)), updated_at INTEGER NOT NULL, PRIMARY KEY(usn,music_id)) STRICT, WITHOUT ROWID;
CREATE TABLE music_scores(usn INTEGER NOT NULL REFERENCES save_slots(usn) ON DELETE CASCADE, music_id INTEGER NOT NULL, difficulty INTEGER NOT NULL, score INTEGER NOT NULL DEFAULT 0, grade INTEGER NOT NULL DEFAULT 0, play_count INTEGER NOT NULL DEFAULT 0, combo_grade INTEGER NOT NULL DEFAULT 0, gp INTEGER NOT NULL DEFAULT 0, multiple REAL NOT NULL DEFAULT 1, achievement INTEGER NOT NULL DEFAULT 0, play_type INTEGER NOT NULL DEFAULT 0, played_at TEXT NOT NULL DEFAULT '', mission_clear INTEGER NOT NULL DEFAULT 0, is_credit INTEGER NOT NULL DEFAULT 0, updated_at INTEGER NOT NULL, PRIMARY KEY(usn,music_id,difficulty)) STRICT, WITHOUT ROWID;
CREATE TABLE user_collections(usn INTEGER NOT NULL REFERENCES save_slots(usn) ON DELETE CASCADE, music_id INTEGER NOT NULL, value INTEGER NOT NULL DEFAULT 0, get_type INTEGER NOT NULL DEFAULT 0, get_time TEXT NOT NULL DEFAULT '', create_time TEXT NOT NULL DEFAULT '', PRIMARY KEY(usn,music_id)) STRICT, WITHOUT ROWID;

CREATE TABLE pass_anchors(usn INTEGER PRIMARY KEY REFERENCES save_slots(usn) ON DELETE CASCADE, local_day INTEGER NOT NULL, season INTEGER NOT NULL CHECK(season BETWEEN 1 AND 13), utc_offset_minutes INTEGER NOT NULL CHECK(utc_offset_minutes BETWEEN -840 AND 840), updated_at INTEGER NOT NULL) STRICT;
CREATE TABLE pass_progress(usn INTEGER NOT NULL REFERENCES save_slots(usn) ON DELETE CASCADE, season INTEGER NOT NULL CHECK(season BETWEEN 1 AND 13), points INTEGER NOT NULL DEFAULT 0 CHECK(points>=0), step INTEGER NOT NULL DEFAULT 0, version INTEGER NOT NULL DEFAULT 1, PRIMARY KEY(usn,season,version)) STRICT, WITHOUT ROWID;
CREATE TABLE pass_claims(usn INTEGER NOT NULL REFERENCES save_slots(usn) ON DELETE CASCADE, season INTEGER NOT NULL, version INTEGER NOT NULL DEFAULT 1, lane INTEGER NOT NULL CHECK(lane IN(0,1)), step INTEGER NOT NULL, claimed_at INTEGER NOT NULL, PRIMARY KEY(usn,season,version,lane,step)) STRICT, WITHOUT ROWID;
CREATE TABLE pass_entitlements(usn INTEGER NOT NULL REFERENCES save_slots(usn) ON DELETE CASCADE, season INTEGER NOT NULL, premium INTEGER NOT NULL DEFAULT 1 CHECK(premium IN(0,1)), lounge INTEGER NOT NULL DEFAULT 0 CHECK(lounge IN(0,1)), PRIMARY KEY(usn,season)) STRICT, WITHOUT ROWID;

CREATE TABLE mail(usn INTEGER NOT NULL REFERENCES save_slots(usn) ON DELETE CASCADE, post_id INTEGER NOT NULL, subject_key TEXT NOT NULL, body_key TEXT NOT NULL, created_at INTEGER NOT NULL, read_at INTEGER, claimed_at INTEGER, deleted_at INTEGER, PRIMARY KEY(usn,post_id)) STRICT, WITHOUT ROWID;
CREATE TABLE mail_rewards(usn INTEGER NOT NULL REFERENCES save_slots(usn) ON DELETE CASCADE, post_id INTEGER NOT NULL, reward_kind TEXT NOT NULL, reward_id INTEGER, amount REAL, area INTEGER NOT NULL DEFAULT 0, PRIMARY KEY(usn,post_id), FOREIGN KEY(usn,post_id) REFERENCES mail(usn,post_id) ON DELETE CASCADE) STRICT, WITHOUT ROWID;
CREATE TABLE mail_state(usn INTEGER PRIMARY KEY REFERENCES save_slots(usn) ON DELETE CASCADE, cursor INTEGER NOT NULL DEFAULT 0 CHECK(cursor>=0)) STRICT;
CREATE TABLE ticket_collection(usn INTEGER NOT NULL REFERENCES save_slots(usn) ON DELETE CASCADE, ticket_id INTEGER NOT NULL, acquired_at INTEGER NOT NULL, PRIMARY KEY(usn,ticket_id)) STRICT, WITHOUT ROWID;

CREATE TABLE ch3_energy(usn INTEGER PRIMARY KEY REFERENCES save_slots(usn) ON DELETE CASCADE, amount INTEGER NOT NULL DEFAULT 0 CHECK(amount>=0), cap INTEGER NOT NULL DEFAULT 0 CHECK(cap>=0), calculated_at INTEGER NOT NULL) STRICT;
CREATE TABLE ch3_stages(usn INTEGER NOT NULL REFERENCES save_slots(usn) ON DELETE CASCADE, stage_id INTEGER NOT NULL, chapter INTEGER NOT NULL, stage_index INTEGER NOT NULL, completed INTEGER NOT NULL DEFAULT 0 CHECK(completed IN(0,1)), best_star INTEGER NOT NULL DEFAULT 0 CHECK(best_star BETWEEN 0 AND 3), best_score INTEGER NOT NULL DEFAULT 0, unlocked INTEGER NOT NULL DEFAULT 0 CHECK(unlocked IN(0,1)), PRIMARY KEY(usn,stage_id)) STRICT, WITHOUT ROWID;
CREATE TABLE ch3_drops(usn INTEGER NOT NULL REFERENCES save_slots(usn) ON DELETE CASCADE, transaction_id TEXT NOT NULL, roll INTEGER NOT NULL, reward_index INTEGER NOT NULL, reward_kind TEXT NOT NULL, reward_id INTEGER, amount REAL NOT NULL, PRIMARY KEY(usn,transaction_id,roll,reward_index)) STRICT, WITHOUT ROWID;
CREATE TABLE ch3_chapter_claims(usn INTEGER NOT NULL REFERENCES save_slots(usn) ON DELETE CASCADE, chapter INTEGER NOT NULL, reward_num INTEGER NOT NULL, claimed_at INTEGER NOT NULL, PRIMARY KEY(usn,chapter,reward_num)) STRICT, WITHOUT ROWID;

CREATE TABLE purchase_transactions(usn INTEGER NOT NULL REFERENCES save_slots(usn) ON DELETE CASCADE, transaction_id TEXT NOT NULL, purchase_kind TEXT NOT NULL, target_id INTEGER NOT NULL, status TEXT NOT NULL, created_at INTEGER NOT NULL, PRIMARY KEY(usn,transaction_id)) STRICT, WITHOUT ROWID;
CREATE TABLE reward_transactions(usn INTEGER NOT NULL REFERENCES save_slots(usn) ON DELETE CASCADE, transaction_id TEXT NOT NULL, source TEXT NOT NULL, reward_json TEXT NOT NULL CHECK(json_valid(reward_json)), created_at INTEGER NOT NULL, PRIMARY KEY(usn,transaction_id)) STRICT, WITHOUT ROWID;
CREATE TABLE state_revisions(usn INTEGER NOT NULL REFERENCES save_slots(usn) ON DELETE CASCADE, partition TEXT NOT NULL, revision INTEGER NOT NULL DEFAULT 0, last_request_seq INTEGER NOT NULL DEFAULT 0, updated_at INTEGER NOT NULL, PRIMARY KEY(usn,partition)) STRICT, WITHOUT ROWID;
CREATE TABLE mutation_ledger(usn INTEGER NOT NULL REFERENCES save_slots(usn) ON DELETE CASCADE, ledger_id INTEGER NOT NULL, request_seq INTEGER NOT NULL, rpc TEXT NOT NULL, idempotency_key TEXT, committed_at INTEGER NOT NULL, summary_json TEXT NOT NULL CHECK(json_valid(summary_json)), PRIMARY KEY(usn,ledger_id), UNIQUE(usn,idempotency_key)) STRICT, WITHOUT ROWID;

CREATE INDEX idx_mail_visible ON mail(usn,deleted_at,claimed_at,created_at);
CREATE INDEX idx_mutation_seq ON mutation_ledger(usn,request_seq);
