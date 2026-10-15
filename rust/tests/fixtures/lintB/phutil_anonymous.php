<?php

class OuterPhutilClass {
  public function make() {
    return new class {
      public function run() {}
    };
  }
}
