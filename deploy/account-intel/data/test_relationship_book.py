#!/usr/bin/env python3
"""The synthetic relationship book's fixture shape, checked offline.

WHAT THIS ESTABLISHES: that the eight committed files in `relationships/` are the shape the
seeder persists and the seam can render, that their dates lie inside the declared snapshot, and
that they carry synthetic provenance. WHAT IT DOES NOT: anything about whether the analyst
reasons over them correctly. `relationship-intelligence@1` declares `"evaluation": null` on
purpose; no assertion here may be cited as evidence about answer quality.

Three of the checks exist because the failure they catch is SILENT rather than loud:

  * a key inside `account{}` that is not one of the seeder's columns is dropped without an
    error, so a relationship fact written as a sibling of `name` simply never reaches anyone;
  * an authored `contact_id` is ignored — the seeder generates `<record_id>-C<n>` — so a file
    carrying one tells a reader something that is not true of the store;
  * `record_id` and `account.name` are what `context_ingress._usable_catalog_rows` and
    `_usable_context` require: a record missing either is DROPPED from the turn and reported
    only to the operator's log, so the analyst answers from a short book without knowing it.

Stdlib only, no database, no seam import. Run: python3 test_relationship_book.py
"""
import datetime
import json
import pathlib
import re
import unittest

HERE = pathlib.Path(__file__).resolve().parent
BOOK_DIR = HERE / "relationships"

# The date this book is a snapshot of — start of business on it, per GUIDANCE.demo.md, which
# tells the analyst to assess staleness and due items as of that point rather than as of today.
# That is what keeps a committed demo from changing meaning as wall-clock time advances. The
# time of day is not checked here and does not need to be: nothing is dated on the boundary,
# and what happens ON it is a reasoning rule the guidance owns. Moving the date is a deliberate
# edit in both places.
SNAPSHOT = datetime.date(2026, 9, 1)

# The eight relationships, and the distinction each one carries. Named here rather than counted,
# so deleting a file fails with the name of what was lost.
EXPECTED_IDS = {
    "LARK-001":   "recurring overdue obligation + statement without a confirming variation",
    "VIREO-001":  "sequential supersession",
    "HALDEN-001": "unresolved same-day contradiction",
    "OSTARA-001": "evidence-relative staleness",
    "CORVID-001": "expected silence (the negative control)",
    "TESS-001":   "not-yet-due work",
    "NORTH-001":  "belief the record does not carry",
    "MARROW-001": "thin record requiring an honest UNKNOWN",
}

# The account fields THIS book uses. The seeder persists a wider set of columns; anything
# outside that wider set is dropped silently, and anything outside this narrower set would mean
# the book has quietly grown a second shape. Relationship facts go in `facts`.
ACCOUNT_FIELDS = {"name", "domain", "industry", "owner", "facts"}
FACT_KEYS = {"relationship_type"}

ISO_DATE = re.compile(r"^\d{4}-\d{2}-\d{2}$")


def _load():
    books = {}
    for path in sorted(BOOK_DIR.glob("*.json")):
        books[path.name] = json.loads(path.read_text())
    return books


class RelationshipBook(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.books = _load()

    def test_the_book_is_exactly_the_eight_expected_relationships(self):
        self.assertEqual(len(self.books), 8, sorted(self.books))
        ids = {doc["record_id"] for doc in self.books.values()}
        self.assertEqual(ids, set(EXPECTED_IDS), f"missing: {set(EXPECTED_IDS) - ids}")

    def test_each_file_is_an_object_with_the_fields_the_turn_requires(self):
        names = []
        for name, doc in self.books.items():
            self.assertIsInstance(doc, dict, name)
            self.assertIsInstance(doc.get("record_id"), str, name)
            self.assertTrue(doc["record_id"].strip(), name)
            account = doc.get("account")
            self.assertIsInstance(account, dict, name)
            self.assertIsInstance(account.get("name"), str, name)
            self.assertTrue(account["name"].strip(), name)
            names.append(account["name"])
        self.assertEqual(len(set(names)), len(names), f"duplicate account names: {names}")

    def test_record_ids_are_unique(self):
        ids = [doc["record_id"] for doc in self.books.values()]
        self.assertEqual(len(set(ids)), len(ids), f"duplicate record_ids: {ids}")

    def test_only_the_intended_account_fields_and_one_declared_fact_are_used(self):
        for name, doc in self.books.items():
            account = doc["account"]
            self.assertLessEqual(set(account), ACCOUNT_FIELDS,
                                 f"{name}: {set(account) - ACCOUNT_FIELDS} would be dropped by "
                                 "the seeder or is a second shape; put facts in `facts`")
            facts = account.get("facts") or {}
            self.assertLessEqual(set(facts), FACT_KEYS, name)
            self.assertIsInstance(facts.get("relationship_type"), str, name)
            self.assertTrue(facts["relationship_type"].strip(), name)

    def test_domains_are_reserved_example_names(self):
        for name, doc in self.books.items():
            domain = doc["account"].get("domain", "")
            self.assertTrue(domain.endswith(".example"), f"{name}: {domain!r}")

    def test_every_file_carries_explicit_synthetic_provenance(self):
        for name, doc in self.books.items():
            marker = doc.get("_synthetic")
            self.assertIsInstance(marker, str, name)
            self.assertIn("SYNTHETIC", marker, name)

    def test_activity_ids_are_unique_across_the_whole_book_and_non_empty(self):
        # `doc["activities"]` here and `doc.get("contacts", [])` below, deliberately: an account
        # may accurately have no contacts (docs/PRODUCT_DIRECTION.md), but a relationship with no
        # dated evidence is not a relationship this service can say anything about.
        seen = []
        for name, doc in self.books.items():
            for act in doc["activities"]:
                self.assertIsInstance(act.get("activity_id"), str, name)
                self.assertTrue(act["activity_id"].strip(), name)
                seen.append(act["activity_id"])
        self.assertEqual(len(set(seen)), len(seen),
                         "duplicate activity_id: the upsert is keyed on it, so a repeat "
                         "OVERWRITES the earlier activity's body instead of adding a row")

    def test_every_activity_carries_a_kind_and_a_body(self):
        for name, doc in self.books.items():
            for act in doc["activities"]:
                self.assertTrue((act.get("kind") or "").strip(), f"{name}: {act['activity_id']}")
                self.assertTrue((act.get("body") or "").strip(), f"{name}: {act['activity_id']}")

    def test_activity_dates_are_iso_and_land_inside_the_snapshot(self):
        for name, doc in self.books.items():
            for act in doc["activities"]:
                raw = act.get("occurred_at")
                self.assertIsInstance(raw, str, name)
                self.assertRegex(raw, ISO_DATE, f"{name}: {act['activity_id']}")
                when = datetime.date.fromisoformat(raw)
                self.assertLessEqual(when, SNAPSHOT,
                                     f"{name}: {act['activity_id']} is dated after the declared "
                                     f"snapshot {SNAPSHOT.isoformat()}")

    def test_activities_are_authored_in_non_decreasing_date_order(self):
        for name, doc in self.books.items():
            dates = [act["occurred_at"] for act in doc["activities"]]
            self.assertEqual(dates, sorted(dates),
                             f"{name}: authored out of order — the store re-sorts, but a file a "
                             "human cannot read in sequence is a file a human will edit wrongly")

    def test_contacts_are_named_and_do_not_author_a_contact_id(self):
        for name, doc in self.books.items():
            for contact in doc.get("contacts", []):
                self.assertTrue((contact.get("name") or "").strip(), name)
                self.assertNotIn("contact_id", contact,
                                 f"{name}: the seeder generates <record_id>-C<n>; an authored "
                                 "contact_id is ignored and misleads whoever reads this file")


if __name__ == "__main__":
    unittest.main(verbosity=2)
