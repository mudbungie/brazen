+++
title = "tests/pipeline_input.rs: use the tempfile dev-dep instead of hand-rolled temp files"
created = 1782204050
updated = 1782205007
claimant = "Lane3"
parent = "bl-3d74"
tags = ["cleanup", "lane3"]
+++
pipeline_input.rs:31-37/58 hand-rolls unique temp files in TMPDIR instead of the tempfile dev-dep used elsewhere. Pure consistency/robustness. FIX: use tempfile NamedTempFile/tempdir.