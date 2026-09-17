import copy
import unittest

from scripts.gui_acceptance import required_tools
from scripts.gui_graphics import FIXED_IDS, PROTOCOLS, graphics_clients_accepted


class GraphicsAcceptanceTests(unittest.TestCase):
    def setUp(self):
        self.records = [{"event": "graphics_client", "tool": tool, "protocol": protocol,
                         "version": "fixture 1", "output_bytes": 100,
                         "image_id": FIXED_IDS.get(tool, 12345), "cleanup_observed": True}
                        for tool, protocol in PROTOCOLS.items()]
        self.log = "\n".join([
            *(f"Accepted Kitty image: id={number}, 64x64 pixels" for number in (1701, 1703, 1704, 12345)),
            "Accepted Kitty Unicode placeholder image: id=1702, 64x64 pixels",
            "Accepted Sixel image: cursor advance 8x4 cells",
            "Accepted iTerm2 inline image: cursor advance 2x1 cells",
        ])

    def accepted(self, records=None, log=None):
        return graphics_clients_accepted(self.records if records is None else records,
                                         (self.log if log is None else log).__contains__)

    def test_requires_exact_client_set_ids_versions_protocols_and_cleanup(self):
        self.assertTrue(self.accepted())
        self.assertFalse(self.accepted(self.records[:-1]))
        self.assertFalse(self.accepted(self.records + [self.records[0]]))
        for index, key, value in [(0, "image_id", 17010), (0, "image_id", "1701"),
                                  (0, "version", ""), (0, "output_bytes", 0),
                                  (0, "output_bytes", True), (0, "protocol", "sixel"),
                                  (2, "cleanup_observed", False), (3, "cleanup_observed", 1),
                                  (4, "image_id", 17), (4, "image_id", 1701),
                                  (4, "image_id", -1), (4, "image_id", 2**32)]:
            records = copy.deepcopy(self.records)
            records[index][key] = value
            with self.subTest(index=index, key=key, value=value):
                self.assertFalse(self.accepted(records))

    def test_log_ids_require_delimiters_and_each_protocol_acceptance(self):
        for message in self.log.splitlines():
            self.assertFalse(self.accepted(log=self.log.replace(message, "")))
        self.assertFalse(self.accepted(log=self.log.replace("id=1701,", "id=17010,")))
        self.assertFalse(self.accepted(log=self.log.replace("id=12345,", "id=123456,")))

    def test_wayland_graphics_declares_all_clients_and_private_input_tools(self):
        tools = required_tools("wayland", ["graphics"], False, True)
        self.assertEqual(set(tools), {"tic", "kitten", "img2sixel", "timg", "wtype", "sway", "swaymsg"})


if __name__ == "__main__":
    unittest.main()
