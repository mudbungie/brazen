+++
title = "Bundle data-plane seams into a RunIo/Host struct; drop the too_many_arguments allow"
created = 1782204026
updated = 1782205270
claimant = "Lane1"
parent = "bl-3d74"
tags = ["cleanup", "lane1"]
+++
run/mod.rs:34 and serve.rs:24 carry an allow(clippy::too_many_arguments) passing 8-9 loose seams, while sibling verbs already bundle theirs (models.rs:29 ListIo, login.rs:52 LoginIo). FIX: introduce a RunIo/Host struct bundling transport/store/cache/clock + writers; thread it through run/serve; delete the clippy suppression; update the main.rs call site. Keep 100% line coverage + clippy clean. Largest item — do it LAST in the lane so it rebases on the doc/help fixes.