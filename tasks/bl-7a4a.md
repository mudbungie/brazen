+++
title = "Pin the release-plz version-bump policy in release-plz.toml"
created = 1784698899
updated = 1784698899
root_commit = "5969984c7c332086256b0e88bf4c438431e9946f"
+++
brazen relied on release-plz's implicit default increment with no config file; add release-plz.toml so the patch-by-default policy and the [minor]/[major] opt-in markers are explicit and match the other repos (balls/lernie/yog/frot).