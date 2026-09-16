CREATE TABLE gp_balances(
  usn INTEGER NOT NULL REFERENCES save_slots(usn) ON DELETE CASCADE,
  kind TEXT NOT NULL CHECK(kind IN ('ch1_like','ch2_like','ch2_note')),
  amount TEXT NOT NULL CHECK(length(amount) BETWEEN 1 AND 4096
    AND amount NOT GLOB '*[^0-9]*' AND (amount='0' OR substr(amount,1,1)<>'0')),
  PRIMARY KEY(usn,kind)
) STRICT, WITHOUT ROWID;
