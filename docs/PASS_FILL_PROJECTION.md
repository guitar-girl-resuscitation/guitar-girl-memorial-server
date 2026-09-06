# Effective Star Pass fill quantities

Keep the source master catalog unchanged. The paid/ad command handlers already
multiply source points by the memorial multiplier once when granting points.

The client also consumes the `getGameDataList` catalog, which can replace values
from the patched local bundle. Both projections must publish the effective
`SubscribePass.paidpoint` and `adpoint`; `pointprice` remains the source price.
Do not apply the display multiplier to progress goals or multiply awards twice.

`getUpdateTime` overlay generation 10 invalidates existing generation-9 catalogs
without clearing player saves. Changing only the bundle or the projection leaves
old display values cached in an installed game.

Samsung acceptance, same-signature 800013 -> 800014 upgrade without clearing data:
the fill dialog changed from 5,000/2,000 to 50,000/20,000, cost stayed 50 chocolate,
and season 7 progress stayed Lv13 with 8,000/14,000. A previous actual paid fill
advanced the progress by exactly 50,000. Spanish locale survived the upgrade.

The master projection regression verifies both fields, unchanged price and source
row. The private bundle/catalog equivalence fixture compares 54,371 scalar
fields across 38 tables; original master resources are not committed here.
