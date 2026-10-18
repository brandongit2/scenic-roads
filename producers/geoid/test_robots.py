import unittest

from robots import allowed

P = "/us_nga_egm08_25.tif"


class Robots(unittest.TestCase):
    def test_empty_or_unrelated_allows(self):
        self.assertTrue(allowed("", P))
        self.assertTrue(allowed("User-agent: otherbot\nDisallow: /\n", P))
        self.assertTrue(allowed("User-agent: *\nDisallow:\n", P))

    def test_star_group_disallows(self):
        self.assertFalse(allowed("User-agent: *\nDisallow: /\n", P))
        self.assertFalse(allowed("User-agent: *\nDisallow: /us_\n", P))
        self.assertTrue(allowed("User-agent: *\nDisallow: /ca_\n", P))

    def test_crlf_and_comments(self):
        self.assertFalse(allowed("User-agent: *\r\nDisallow: /us_ # no\r\n", P))
        self.assertFalse(allowed("USER-AGENT: *\rDISALLOW: /\r", P))

    def test_consecutive_agents_form_one_group(self):
        text = "User-agent: a\nUser-agent: *\nDisallow: /\n\nUser-agent: b\nAllow: /\n"
        self.assertFalse(allowed(text, P))
        text = "User-agent: *\nDisallow: /x\nUser-agent: b\nDisallow: /\n"
        self.assertTrue(allowed(text, P))

    def test_our_group_replaces_the_star_one(self):
        text = "User-agent: *\nDisallow: /\n\nUser-agent: other\nUser-agent: Scenic-Roads\nAllow: /\n"
        self.assertTrue(allowed(text, P))
        text = "User-agent: *\nAllow: /\n\nUser-agent: scenic-roads\nDisallow: /us_\n"
        self.assertFalse(allowed(text, P))
        # Our groups merge.
        text = "User-agent: scenic-roads\nDisallow: /x\n\nUser-agent: scenic-roads\nDisallow: /us\n"
        self.assertFalse(allowed(text, P))

    def test_longest_match_wins_allow_on_ties(self):
        self.assertTrue(allowed("User-agent: *\nDisallow: /\nAllow: /us_nga\n", P))
        self.assertFalse(allowed("User-agent: *\nAllow: /us\nDisallow: /us_nga\n", P))
        self.assertTrue(allowed("User-agent: *\nDisallow: /us\nAllow: /us\n", P))

    def test_wildcards(self):
        self.assertFalse(allowed("User-agent: *\nDisallow: /*.tif$\n", P))
        self.assertTrue(allowed("User-agent: *\nDisallow: /*.tif$\n", P + "?x"))
        self.assertFalse(allowed("User-agent: *\nDisallow: /*egm08\n", P))
        self.assertTrue(allowed("User-agent: *\nDisallow: /*.gtx$\n", P))


if __name__ == "__main__":
    unittest.main()
