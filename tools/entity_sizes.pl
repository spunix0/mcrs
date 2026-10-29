# Usage: perl entity_sizes.pl EntityTypes.java > entity_sizes.tsv
# Extracts (entity, width, height, category) from 26.3's EntityTypes.java.
use strict; use warnings;
local $/; my $src = <>;
print "# entity\twidth\theight\tcategory — from 26.3 EntityTypes.java\n";
my @decls = split /\n   public static final EntityType<[^>]*> /, $src; shift @decls;
for my $d (@decls) {
  my ($name) = $d =~ /^(\w+) = register/ or next;
  ($d) = split /\n   (?:public|private|static)/, $d;
  my ($w, $h) = $d =~ /\.sized\(([\d.]+)F, ([\d.]+)F\)/ ? ($1, $2) : (0.6, 1.8);
  my ($cat) = $d =~ /MobCategory\.(\w+)/; $cat //= 'MISC';
  printf "%s\t%s\t%s\t%s\n", lc $name, $w, $h, lc $cat;
}
