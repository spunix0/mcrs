#!/usr/bin/perl
# Extracts the creative inventory tabs from the decompiled CreativeModeTabs.java.
# Usage: perl creative_tabs.pl <net/minecraft dir> > ../crates/mc-assets/data/creative_tabs.tsv
# Output lines: <tab key>\t<kind>\t<NAME>, kinds: item (Items.NAME), pick (Items.FAMILY.method(): name in 2nd col
# as FAMILY:method), colored (16 dye colors of a family), family (every item in a copper/dyed family).
use strict;
use warnings;
my $dir = shift or die "usage: $0 <net/minecraft dir>\n";
open my $f, '<', "$dir/world/item/CreativeModeTabs.java" or die "$!\n";
my $tab;
while (my $line = <$f>) {
    if ($line =~ /translatable\("itemGroup\.(\w+)"\)/) {
        $tab = $1;
        next;
    }
    next unless defined $tab;
    if ($line =~ /registerColoredItems\(\w+, \w+, Items\.(\w+)\)/) {
        print "$tab\tcolored\t$1\n";
    } elsif ($line =~ /Items\.DYE\.forEach/) {
        print "$tab\tcolored\tDYE\n";
    } elsif ($line =~ /Items\.(\w+)\.waxed\(\)\.forEach/) {
        print "$tab\tfamily\tWAXED_$1\n";
    } elsif ($line =~ /Items\.(\w+)\.forEach/) {
        print "$tab\tfamily\t$1\n";
    } else {
        while ($line =~ /accept\(Items\.(\w+)(?:\.(\w+)\(\))?/g) {
            print defined $2 ? "$tab\tpick\t$1:$2\n" : "$tab\titem\t$1\n";
        }
    }
}
