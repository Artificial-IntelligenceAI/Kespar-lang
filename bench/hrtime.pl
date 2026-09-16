#!/usr/bin/perl
# usage: hrtime.pl <stdin-file> cmd args...   — prints wall seconds of the command
use Time::HiRes qw(time);
my $in = shift @ARGV;
open(STDIN, "<", $in) or die; open(STDOUT, ">", "/dev/null");
my $t = time; system(@ARGV); printf STDERR "%.4f", time - $t;
