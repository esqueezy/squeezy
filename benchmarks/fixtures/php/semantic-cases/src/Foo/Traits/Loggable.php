<?php

declare(strict_types=1);

namespace Foo\Traits;

trait Loggable
{
    protected function log(string $message): void
    {
        // Intentionally trivial; the body exists so squeezy emits a
        // `body_span` for the method and the `log` text shows up in body
        // search hits.
        $this->lastMessage = $message;
    }

    private string $lastMessage = '';
}
